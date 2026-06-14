use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ServerRS::{api, application, bootstrap, domain, infrastructure, shared};
use application::agent::agent_context::AgentContextBuilder;
use application::agent::agent_runtime::AgentRuntime;
use application::community::community_service::CommunityService;
use application::depression::depression_service::DepressionService;
use application::diary::diary_service::DiaryService;
use application::memory::memory_extractor::MemoryExtractor;
use application::memory::memory_service::MemoryService;
use application::music::music_service::MusicService;
use application::psychology::psychology_service::PsychologyService;
use application::rag::chunking::ChunkingService;
use application::rag::ingestion_service::IngestionService;
use application::rag::retrieval_service::RetrievalService;
use application::session::chat_service::ChatService;
use application::session::risk_detection_service::RiskDetectionService;
use application::session::session_service::SessionService;
use application::storage::object_service::ObjectService;
use application::user::user_service::UserService;
use domain::auth::refresh_token_store::RefreshTokenStore;
use domain::conversation::conversation_repository::ConversationRepository;
use domain::llm::{EmbeddingProvider, LlmClient, LlmProvider};
use domain::risk::risk_detector::RiskDetector;
use domain::storage::ObjectStorage;
use domain::tasks::task_handler::TaskHandler;
use domain::tasks::task_publisher::TaskPublisher;
use infrastructure::detector::rule_based_detector::RuleBasedRiskDetector;
use infrastructure::llm::ollama_client::OllamaClient;
use infrastructure::llm::ollama_provider::OllamaProvider;
use infrastructure::persistence::seaorm_db::init_db;
use infrastructure::storage::local_storage::LocalObjectStorage;
use infrastructure::tasks::alert_handler::{AlertConfig, AlertHandler};
use infrastructure::tasks::in_memory_task_flow::new_task_channel;
use infrastructure::tasks::logging_handler::LoggingHandler;
use infrastructure::tasks::rate_limit_handler::{RateLimitConfig, RateLimitHandler};

use shared::config::AppConfig;
use tracing::info;

// ── Agent tool registry ────────────────────────────────────────────────────
use application::agent::tool_registry::{AgentToolDeps, build_default_agent_tools};

#[tokio::main]
async fn main() {
    let config = AppConfig::load();
    init_tracing(&config.logging.level);
    if let Err(err) = run(config).await {
        tracing::error!(error = %err, "server stopped with error");
    }
}

async fn run(config: AppConfig) -> Result<(), std::io::Error> {
    let db = init_db(&config.database.url, config.database.max_connections)
        .await
        .expect("db init");

    // ── Repositories ──
    let repos = bootstrap::repos::build_repos(&db);

    let user_repo = Arc::clone(&repos.user_repo);
    let profile_repo = Arc::clone(&repos.profile_repo);
    let conv_repo = Arc::clone(&repos.conv_repo);
    let risk_repo = Arc::clone(&repos.risk_repo);
    let psychology_repo = Arc::clone(&repos.psychology_repo);
    let depression_repo = Arc::clone(&repos.depression_repo);
    let diary_repo = Arc::clone(&repos.diary_repo);
    let music_repo = Arc::clone(&repos.music_repo);
    let community_repo = Arc::clone(&repos.community_repo);
    let agent_event_repo = Arc::clone(&repos.agent_event_repo);
    let stored_object_repo = Arc::clone(&repos.stored_object_repo);
    let rag_repo = Arc::clone(&repos.rag_repo);
    let memory_repo = Arc::clone(&repos.memory_repo);
    let summary_repo = Arc::clone(&repos.summary_repo);

    // ── Tasks ──
    let mut background = bootstrap::tasks::BackgroundTasks::new();

    let alert_handler = Arc::new(AlertHandler::new(AlertConfig::default()));
    let rate_limit_handler = Arc::new(RateLimitHandler::new(
        RateLimitConfig::default(),
        Arc::clone(&user_repo),
    ));

    let (tp, tw) = new_task_channel(256);
    background.spawn(tokio::spawn(
        tw.with_handler(Arc::new(LoggingHandler))
            .with_handler(Arc::clone(&alert_handler) as Arc<dyn TaskHandler>)
            .with_handler(Arc::clone(&rate_limit_handler) as Arc<dyn TaskHandler>)
            .run(),
    ));
    let task_publisher: Arc<dyn TaskPublisher> = Arc::new(tp);

    // Periodic cleanup for stateful handlers
    background.spawn({
        let h = Arc::clone(&alert_handler);
        tokio::spawn(async move {
            let mut i = tokio::time::interval(tokio::time::Duration::from_secs(300));
            loop {
                i.tick().await;
                h.cleanup().await;
            }
        })
    });
    background.spawn({
        let h = Arc::clone(&rate_limit_handler);
        tokio::spawn(async move {
            let mut i = tokio::time::interval(tokio::time::Duration::from_secs(120));
            loop {
                i.tick().await;
                h.cleanup().await;
            }
        })
    });

    // ── Auth infra ──
    let auth_graph =
        bootstrap::auth::build_auth(&db, &config.jwt, &config.auth, &user_repo, &task_publisher);
    background.spawn({
        let store = Arc::clone(&auth_graph.refresh_token_store);
        tokio::spawn(periodic_revocation(store))
    });

    // ── LLM (infrastructure → domain trait) ──
    // Legacy LlmClient (used by DiaryService for title generation)
    let ollama_client: Arc<dyn LlmClient> = Arc::new(OllamaClient::new(
        config.ollama.base_url.clone(),
        config.ollama.model.clone(),
        config.ollama.temperature,
        config.ollama.top_p,
    ));
    // ── Chat LLM provider (Agent uses config.llm.*) ──
    let ollama_provider: Arc<dyn LlmProvider> = Arc::new(OllamaProvider::with_timeout(
        config.llm.base_url.clone(),
        config.llm.chat_model.clone(),
        config.llm.timeout_secs,
    ));
    // ── Dedicated embedding provider (separate from chat LLM) ──
    let embedding_provider: Arc<dyn EmbeddingProvider> = Arc::new(
        infrastructure::llm::ollama_embedding_provider::OllamaEmbeddingProvider::with_options(
            config.embedding.base_url.clone(),
            config.embedding.model.clone(),
            config.embedding.dimension,
            config.embedding.batch_size,
            config.embedding.timeout_secs,
        ),
    );

    // ── Qdrant VectorStore (optional, enabled via config) ──
    use domain::vector_store::VectorStore;
    let vector_store: Option<Arc<dyn VectorStore>> = if config.qdrant.enabled {
        #[cfg(feature = "qdrant")]
        {
            let qdrant = infrastructure::vector_store::qdrant_vector_store::QdrantVectorStore::new(
                &config.qdrant.url,
                config.qdrant.api_key.as_deref(),
            )
            .await
            .map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Qdrant init failed: {e}"),
                )
            })?;
            Some(Arc::new(qdrant) as Arc<dyn VectorStore>)
        }
        #[cfg(not(feature = "qdrant"))]
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "qdrant.enabled=true but binary built without --features qdrant",
            ));
        }
    } else {
        None
    };

    use application::rag::vector_index_service::{VectorIndexConfig, VectorIndexService};
    use domain::vector_index::VectorIndexRepository;

    let vector_index_repo: Arc<dyn VectorIndexRepository> = Arc::new(
        infrastructure::persistence::implementations::seaorm_vector_index_repository::SeaOrmVectorIndexRepository::new(db.clone()),
    );

    let vector_index: Option<Arc<VectorIndexService>> = vector_store.as_ref().map(|vs| {
        Arc::new(VectorIndexService::new(
            Arc::clone(&rag_repo),
            Arc::clone(&memory_repo),
            Arc::clone(&summary_repo),
            Arc::clone(&vector_index_repo),
            Arc::clone(vs),
            Arc::clone(&embedding_provider),
            VectorIndexConfig {
                rag_collection: config.qdrant.rag_collection.clone(),
                memory_collection: config.qdrant.memory_collection.clone(),
                summary_collection: config.qdrant.summary_collection.clone(),
                ..Default::default()
            },
        ))
    });

    // ── Ensure vector collections exist (only when Qdrant is enabled) ──
    if let Some(ref vi) = vector_index {
        vi.ensure_collections().await.map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("ensure_collections failed: {e}"),
            )
        })?;
    }

    // ── Risk detector + detection service ──
    let risk_detector: Arc<dyn RiskDetector> = Arc::new(RuleBasedRiskDetector::new());

    let risk_detection_service = Arc::new(RiskDetectionService::new(
        Arc::clone(&risk_repo),
        Arc::clone(&task_publisher),
        Arc::clone(&risk_detector),
    ));

    // ── Services ──
    let auth = Arc::clone(&auth_graph.auth_service);
    let user: Arc<UserService> = Arc::new(UserService::new(
        Arc::clone(&user_repo),
        Arc::clone(&profile_repo),
    ));
    let query: Arc<SessionService> = Arc::new(SessionService::new(
        Arc::clone(&conv_repo),
        Arc::clone(&risk_repo),
    ));

    // ── RAG & Memory services (constructed early — needed by AgentContextBuilder) ──
    let mut retrieval_svc =
        RetrievalService::new(Arc::clone(&rag_repo), Some(Arc::clone(&embedding_provider)))
            .with_hybrid_weights(
                config.rag.hybrid_vector_weight,
                config.rag.hybrid_keyword_weight,
            );
    if let Some(ref vs) = vector_store {
        retrieval_svc =
            retrieval_svc.with_vector_store(Arc::clone(vs), config.qdrant.rag_collection.clone());
    }
    // Surface published web-ingestion content (only when web ingestion is on).
    // Staged/superseded versions are excluded by the active filter + MySQL
    // status re-validation; legacy RAG is unaffected.
    if config.web_ingestion.enabled {
        retrieval_svc =
            retrieval_svc.with_web_collection(config.web_ingestion.qdrant_collection.clone());
    }
    let retrieval: Arc<RetrievalService> = Arc::new(retrieval_svc);

    let chunking = ChunkingService::new();
    let mut ingestion_svc = IngestionService::new(
        Arc::clone(&rag_repo),
        chunking,
        Some(Arc::clone(&embedding_provider)),
    )
    .with_chunking_config(config.rag.chunk_size, config.rag.chunk_overlap);
    if let Some(ref vi) = vector_index {
        ingestion_svc = ingestion_svc.with_vector_index(Arc::clone(vi));
    }
    let ingestion: Arc<IngestionService> = Arc::new(ingestion_svc);

    let memory_extractor: Arc<MemoryExtractor> =
        Arc::new(MemoryExtractor::new(Arc::clone(&ollama_provider)));
    let mut memory_svc = MemoryService::new(Arc::clone(&memory_repo), memory_extractor);
    if let Some(ref vs) = vector_store {
        memory_svc = memory_svc.with_vector_search(
            Arc::clone(vs),
            Arc::clone(&embedding_provider),
            config.qdrant.memory_collection.clone(),
        );
    }
    if let Some(ref vi) = vector_index {
        memory_svc = memory_svc.with_vector_index(Arc::clone(vi));
    }
    let memory_svc: Arc<MemoryService> = Arc::new(memory_svc);

    // ── SummaryService ──
    use application::summary::summary_service::SummaryService;
    let summary_service: Arc<SummaryService> = Arc::new(SummaryService::new(
        Arc::clone(&summary_repo),
        vector_index.clone(),
    ));

    // ── Agent Runtime ──
    let context_builder: Arc<AgentContextBuilder> = Arc::new(AgentContextBuilder::new(
        Arc::clone(&memory_svc),
        Arc::clone(&retrieval),
        Arc::clone(&summary_service),
        Arc::clone(&conv_repo),
        Arc::clone(&profile_repo),
    ));

    let tool_deps = AgentToolDeps {
        retrieval: Arc::clone(&retrieval),
        memory: Arc::clone(&memory_svc),
        diary_repo: Arc::clone(&diary_repo),
        depression_repo: Arc::clone(&depression_repo),
        music_repo: Arc::clone(&music_repo),
        community_repo: Arc::clone(&community_repo),
        agent_event_repo: Arc::clone(&agent_event_repo),
        plugins: config.plugins.clone(),
    };

    let agent_tools = build_default_agent_tools(&tool_deps, config.agent.enabled)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

    let agent_settings = application::agent::agent_runtime::AgentRuntimeSettings {
        agent_enabled: config.agent.enabled,
        memory_enabled: config.agent.memory_enabled,
        rag_enabled: config.agent.rag_enabled,
        summary_enabled: config.agent.summary_enabled,
        max_context_messages: config.agent.max_context_messages as usize,
        max_memory_items: config.agent.max_memory_items,
        max_rag_chunks: config.agent.max_rag_chunks as u64,
        memory_extraction_async: config.agent.memory_extraction_async,
        summary_async: config.agent.summary_async,
        max_tool_depth: config.llm.max_tool_depth as usize,
        temperature: config.llm.temperature,
        top_p: config.llm.top_p,
    };

    let agent_runtime: Arc<AgentRuntime> = Arc::new(AgentRuntime::new(
        Arc::clone(&ollama_provider),
        Arc::clone(&memory_svc),
        Arc::clone(&risk_detection_service),
        Arc::clone(&agent_event_repo),
        Arc::clone(&conv_repo),
        Arc::clone(&profile_repo),
        context_builder,
        Arc::clone(&summary_service),
        agent_tools,
        agent_settings,
    ));

    // ── ChatService (the business entry point) ──
    // Must be built AFTER agent_runtime, which it depends on.
    let chat_service: Arc<ChatService> = Arc::new(ChatService::new(
        Arc::clone(&task_publisher),
        Arc::clone(&conv_repo) as Arc<dyn ConversationRepository>,
        Arc::clone(&agent_runtime),
    ));

    // ── Domain services with real SeaORM repositories ──
    let psychology: Arc<PsychologyService> =
        Arc::new(PsychologyService::new(Arc::clone(&psychology_repo)));
    let depression: Arc<DepressionService> =
        Arc::new(DepressionService::new(Arc::clone(&depression_repo)));
    let diaries: Arc<DiaryService> = Arc::new(DiaryService::new(
        Arc::clone(&diary_repo),
        Some(Arc::clone(&ollama_client)),
    ));
    let local_storage: Arc<dyn ObjectStorage> = Arc::new(LocalObjectStorage::new(
        std::path::PathBuf::from(&config.storage.base_path),
    ));
    let music: Arc<MusicService> = Arc::new(MusicService::new(Arc::clone(&music_repo)));
    let community: Arc<CommunityService> =
        Arc::new(CommunityService::new(Arc::clone(&community_repo)));
    let objects: Arc<ObjectService> = Arc::new(ObjectService::new(
        Arc::clone(&local_storage),
        Arc::clone(&stored_object_repo),
        config.storage.clone(),
    ));

    // ── Web Ingestion ──────────────────────────────────────────────────
    // The review service remains available for inspection when workers are
    // disabled; submitting a publish request then returns a conflict.
    let knowledge_review = bootstrap::web_ingestion::init_web_ingestion(
        &config,
        &db,
        &vector_store,
        &embedding_provider,
        &rag_repo,
        &mut background,
    )
    .await
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

    // ── API ──
    let services = bootstrap::state::ServiceGraph {
        auth,
        user,
        query,
        objects,
        psychology,
        depression,
        diaries,
        music,
        community,
        retrieval,
        ingestion,
        memory: memory_svc,
        agent_runtime,
        knowledge_review,
        chat: chat_service,
        chat_conv_repo: Arc::clone(&conv_repo) as Arc<dyn ConversationRepository>,
    };

    let state = bootstrap::state::build_state(&services);
    let app = api::router::build_router_with_origins(state, &config.cors.allowed_origins)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("server listening on http://{addr}");

    let r = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;
    background.abort_all();
    r
}

async fn periodic_revocation(repo: Arc<dyn RefreshTokenStore>) {
    let mut t = tokio::time::interval(tokio::time::Duration::from_secs(60));
    loop {
        t.tick().await;
        let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_secs(),
            Err(e) => {
                tracing::warn!(error = %e, "clock");
                continue;
            }
        };
        match repo.cleanup_expired(now).await {
            Ok(n) if n > 0 => tracing::info!(n, "expired tokens cleaned"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "cleanup failed"),
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

fn init_tracing(configured_level: &str) {
    let env_filter = std::env::var("RUST_LOG").unwrap_or_default();
    let combined = if env_filter.is_empty() {
        format!("{configured_level},sqlx=warn")
    } else if env_filter.contains("sqlx") {
        // User explicitly set sqlx level — respect it.
        env_filter
    } else {
        // Append sqlx=warn so sqlx query logs are suppressed by default.
        format!("{},sqlx=warn", env_filter)
    };
    let f = tracing_subscriber::EnvFilter::new(&combined);
    tracing_subscriber::fmt()
        .with_env_filter(f)
        .with_target(true)
        // .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .compact()
        .init();
}
