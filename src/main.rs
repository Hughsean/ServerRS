use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use app::agent::agent_context::AgentContextBuilder;
use app::agent::agent_runtime::AgentRuntime;
use app::community::community_service::CommunityService;
use app::depression::depression_service::DepressionService;
use app::diary::diary_service::DiaryService;
use app::memory::memory_extractor::MemoryExtractor;
use app::memory::memory_service::MemoryService;
use app::music::music_service::MusicService;
use app::psychology::psychology_service::PsychologyService;
use app::rag::chunking::ChunkingService;
use app::rag::ingestion_service::IngestionService;
use app::rag::retrieval_service::RetrievalService;
use app::risk::post_conversation_risk_audit_worker::PostConversationRiskAuditWorker;
use app::risk::risk_detection_service::RiskDetectionService;
use app::session::chat_service::ChatService;
use app::session::session_service::SessionService;
use app::storage::object_service::ObjectService;
use app::user::user_service::UserService;
use domain::auth::refresh_token_store::RefreshTokenStore;
use domain::conversation::conversation_repository::ConversationRepository;
use domain::llm::{EmbeddingProvider, LlmClient, LlmProvider};
use domain::risk::risk_detector::RiskDetector;
use domain::storage::ObjectStorage;
use domain::tasks::task_handler::TaskHandler;
use domain::tasks::task_publisher::TaskPublisher;
use infra::detector::rule_based_detector::RuleBasedRiskDetector;
use infra::llm::ollama_client::OllamaClient;
use infra::llm::ollama_provider::OllamaProvider;
use infra::persistence::seaorm_db::init_db;
use infra::storage::local_storage::LocalObjectStorage;
use infra::tasks::alert_handler::{AlertConfig, AlertHandler};
use infra::tasks::in_memory_task_flow::{RetryingTaskPublisher, new_task_channel};
use infra::tasks::logging_handler::LoggingHandler;
use infra::tasks::rate_limit_handler::{RateLimitConfig, RateLimitHandler};
use server_rs::{api, app, bootstrap, domain, infra, shared};

use shared::config::AppConfig;
use tracing::info;

// ── Agent tool registry ────────────────────────────────────────────────────
use app::agent::tool_registry::{AgentToolDeps, build_default_agent_tools};

#[tokio::main]
async fn main() {
    let config = AppConfig::load();
    init_tracing(&config.logging.level);
    if let Err(err) = run(config).await {
        tracing::error!(error = %err, "服务器运行出错");
    }
}

async fn run(config: AppConfig) -> Result<(), std::io::Error> {
    let db = init_db(&config.database.url, config.database.max_connections)
        .await
        .expect("db init");

    // ── 仓库 ──
    let repos = bootstrap::repos::build_repos(
        &db,
        &config.qdrant.memory_collection,
        &config.qdrant.summary_collection,
    );

    let user_repo = Arc::clone(&repos.user_repo);
    let profile_repo = Arc::clone(&repos.profile_repo);
    let context_version_repo = Arc::clone(&repos.context_version_repo);
    let context_control_repo = Arc::clone(&repos.context_control_repo);
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

    // ── 任务系统 ──
    let mut background = bootstrap::tasks::BackgroundTasks::new();

    let alert_handler = Arc::new(AlertHandler::new(AlertConfig::default()));
    let rate_limit_handler = Arc::new(RateLimitHandler::new(
        RateLimitConfig::default(),
        Arc::clone(&user_repo),
    ));

    let (tp, tw) = new_task_channel(256);
    // 启动内存重试队列的后台协程，用于处理发送失败的事件
    let _retry_handle = RetryingTaskPublisher::spawn_retry_worker(tp.clone());
    let task_worker = tw
        .with_handler(Arc::new(LoggingHandler))
        .with_handler(Arc::clone(&alert_handler) as Arc<dyn TaskHandler>)
        .with_handler(Arc::clone(&rate_limit_handler) as Arc<dyn TaskHandler>);
    let task_publisher: Arc<dyn TaskPublisher> = Arc::new(tp);

    // 有状态处理器的定期清理
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

    // ── 认证基础设施 ──
    let auth_graph =
        bootstrap::auth::build_auth(&db, &config.jwt, &config.auth, &user_repo, &task_publisher);
    background.spawn({
        let store = Arc::clone(&auth_graph.refresh_token_store);
        tokio::spawn(periodic_revocation(store))
    });

    // ── LLM（基础设施层 → 领域层接口）──
    // 旧版 LlmClient（DiaryService 用于标题生成）
    let ollama_client: Arc<dyn LlmClient> = Arc::new(OllamaClient::new(
        config.ollama.base_url.clone(),
        config.ollama.model.clone(),
        config.ollama.temperature,
        config.ollama.top_p,
    ));
    // ── Chat LLM Provider（Agent 使用 config.llm.*）──
    let ollama_provider: Arc<dyn LlmProvider> = Arc::new(OllamaProvider::with_timeout(
        config.llm.base_url.clone(),
        config.llm.chat_model.clone(),
        config.llm.timeout_secs,
    ));
    // ── 专用 Embedding Provider（与 Chat LLM 分离）──
    let embedding_provider: Arc<dyn EmbeddingProvider> = Arc::new(
        infra::llm::ollama_embedding_provider::OllamaEmbeddingProvider::with_options(
            config.embedding.base_url.clone(),
            config.embedding.model.clone(),
            config.embedding.dimension,
            config.embedding.batch_size,
            config.embedding.timeout_secs,
        ),
    );

    // ── Qdrant 向量存储（可选，通过配置启用）──
    use domain::vector_store::VectorStore;
    let vector_store: Option<Arc<dyn VectorStore>> = if config.qdrant.enabled {
        #[cfg(feature = "qdrant")]
        {
            let qdrant = infra::vector_store::qdrant_vector_store::QdrantVectorStore::new(
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

    use app::rag::vector_index_service::{VectorIndexConfig, VectorIndexService};
    use domain::vector_index::VectorIndexRepository;

    let vector_index_repo: Arc<dyn VectorIndexRepository> = Arc::new(
        infra::persistence::implementations::seaorm_vector_index_repository::SeaOrmVectorIndexRepository::new(db.clone()),
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

    // ── 确保向量集合已存在（仅在 Qdrant 启用时）──
    if let Some(ref vi) = vector_index {
        vi.ensure_collections().await.map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("ensure_collections failed: {e}"),
            )
        })?;
    }

    // ── 风险检测器 + 检测服务 ──
    let risk_detector: Arc<dyn RiskDetector> = Arc::new(RuleBasedRiskDetector::new());

    let risk_detection_service = Arc::new(RiskDetectionService::new(
        Arc::clone(&risk_repo),
        Arc::clone(&task_publisher),
        Arc::clone(&risk_detector),
    ));
    let risk_audit_worker: Arc<dyn TaskHandler> = Arc::new(PostConversationRiskAuditWorker::new(
        Arc::clone(&conv_repo),
        Arc::clone(&risk_detection_service),
    ));

    // ── 服务 ──
    let auth = Arc::clone(&auth_graph.auth_service);
    let user: Arc<UserService> = Arc::new(UserService::new(
        Arc::clone(&user_repo),
        Arc::clone(&profile_repo),
    ));
    let query: Arc<SessionService> = Arc::new(SessionService::new(
        Arc::clone(&conv_repo),
        Arc::clone(&risk_repo),
    ));

    // ── RAG 与记忆服务（提前构建 — AgentContextBuilder 需要它们）──
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
    // 公开已发布的网页知识摄取内容（仅在网页摄取启用时）
    // 已暂存/已取代的版本被活跃过滤器 + MySQL 状态重新验证排除；
    // 旧版 RAG 不受影响。
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
    let mut memory_svc = MemoryService::new(Arc::clone(&memory_repo), memory_extractor)
        .with_personalization_profile_repo(Arc::clone(&profile_repo))
        .with_context_version_repo(Arc::clone(&context_version_repo));
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
    use app::summary::summary_service::SummaryService;
    let summary_service: Arc<SummaryService> = Arc::new(SummaryService::new(
        Arc::clone(&summary_repo),
        vector_index.clone(),
    ));
    use app::summary::summary_refresh_handler::SummaryRefreshHandler;
    let summary_refresh_handler: Arc<dyn TaskHandler> = Arc::new(SummaryRefreshHandler::new(
        config.agent.enabled && config.agent.summary_enabled && config.agent.summary_async,
        Arc::clone(&ollama_provider) as Arc<dyn LlmProvider>,
        Arc::clone(&conv_repo) as Arc<dyn ConversationRepository>,
        Arc::clone(&summary_service),
        Arc::clone(&context_version_repo),
    ));
    background.spawn(tokio::spawn(
        task_worker
            .with_handler(risk_audit_worker)
            .with_handler(summary_refresh_handler)
            .run(),
    ));

    // ── 代理运行时 ──
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
        plugins: config.plugins.clone(),
    };

    let agent_tools = build_default_agent_tools(&tool_deps, config.agent.enabled)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

    let agent_settings = app::agent::agent_runtime::AgentRuntimeSettings {
        agent_enabled: config.agent.enabled,
        memory_enabled: config.agent.memory_enabled,
        rag_enabled: config.agent.rag_enabled,
        summary_enabled: config.agent.summary_enabled,
        max_context_messages: config.agent.max_context_messages as usize,
        max_memory_items: config.agent.max_memory_items,
        max_rag_chunks: config.agent.max_rag_chunks as u64,
        memory_extraction_async: config.agent.memory_extraction_async,
        max_tool_depth: config.llm.max_tool_depth as usize,
        temperature: config.llm.temperature,
        top_p: config.llm.top_p,
        enable_reasoning: config.llm.enable_reasoning,
    };

    let agent_runtime: Arc<AgentRuntime> = Arc::new(AgentRuntime::new(
        Arc::clone(&ollama_provider),
        Arc::clone(&memory_svc),
        Arc::clone(&agent_event_repo),
        Arc::clone(&conv_repo),
        Arc::clone(&profile_repo),
        Arc::clone(&context_version_repo),
        context_builder,
        agent_tools,
        agent_settings,
    ));

    // ── ChatService（业务入口点）──
    // 必须在 agent_runtime 之后构建，因为它依赖于 agent_runtime。
    let chat_service: Arc<ChatService> = Arc::new(ChatService::new(
        Arc::clone(&task_publisher),
        Arc::clone(&conv_repo) as Arc<dyn ConversationRepository>,
        Arc::clone(&agent_runtime),
        Arc::clone(&memory_svc),
        Arc::clone(&context_control_repo),
        vector_index.clone(),
    ));

    // ── 使用真实 SeaORM 仓库的领域服务 ──
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

    #[cfg(feature = "qq_bot")]
    {
        // ── QQ Bot (赛博猫猫) ──────────────────────────────────────────────
        use crate::bootstrap::qq_bot::init_qq_bot;
        use crate::domain::tts::TtsProvider;
        use crate::infra::qq_bot::repositories::seaorm_agent_turn_repository::SeaOrmAgentTurnRepository;
        use crate::infra::qq_bot::repositories::seaorm_bot_account_repository::SeaOrmBotAccountRepository;
        use crate::infra::qq_bot::repositories::seaorm_external_user_repository::SeaOrmExternalUserRepository;
        use crate::infra::qq_bot::repositories::seaorm_group_member_repository::SeaOrmGroupMemberRepository;
        use crate::infra::qq_bot::repositories::seaorm_group_memory_repository::SeaOrmGroupMemoryRepository;
        use crate::infra::qq_bot::repositories::seaorm_group_message_repository::SeaOrmGroupMessageRepository;
        use crate::infra::qq_bot::repositories::seaorm_group_repository::SeaOrmGroupRepository;
        use crate::infra::qq_bot::repositories::seaorm_group_summary_repository::SeaOrmGroupSummaryRepository;
        use crate::infra::qq_bot::repositories::seaorm_outbox_repository::SeaOrmOutboxRepository;
        use crate::infra::qq_bot::repositories::seaorm_user_profile_repository::SeaOrmQqUserProfileRepository;
        use crate::infra::tts::volcengine_provider::VolcengineTtsProvider;

        use crate::infra::qq_bot::repositories::seaorm_relationship_repository::SeaOrmRelationshipRepository;

        let qq_bot_bot_account_repo = Arc::new(SeaOrmBotAccountRepository::new(db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::BotAccountRepository>;
        let qq_bot_group_repo = Arc::new(SeaOrmGroupRepository::new(db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::GroupRepository>;
        let qq_bot_group_member_repo = Arc::new(SeaOrmGroupMemberRepository::new(db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::GroupMemberRepository>;
        let qq_bot_group_message_repo = Arc::new(SeaOrmGroupMessageRepository::new(db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::GroupMessageRepository>;
        let qq_bot_group_summary_repo = Arc::new(SeaOrmGroupSummaryRepository::new(db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::GroupSummaryRepository>;
        let qq_bot_group_memory_repo = Arc::new(SeaOrmGroupMemoryRepository::new(db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::GroupMemoryRepository>;
        let qq_bot_agent_turn_repo = Arc::new(SeaOrmAgentTurnRepository::new(db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::AgentTurnRepository>;
        let qq_bot_outbox_repo = Arc::new(SeaOrmOutboxRepository::new(db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::OutboxRepository>;
        let qq_bot_external_user_repo = Arc::new(SeaOrmExternalUserRepository::new(db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::ExternalUserRepository>;
        let qq_bot_user_profile_repo = Arc::new(SeaOrmQqUserProfileRepository::new(db.clone()))
            as Arc<dyn crate::domain::qq_bot::qq_profile_repository::QqUserProfileRepository>;
        let qq_bot_relationship_repo = Arc::new(SeaOrmRelationshipRepository::new(db.clone()))
            as Arc<dyn crate::domain::qq_bot::relationship_repository::RelationshipRepository>;

        // QQ 机器人语音消息的 TTS Provider
        let qq_bot_tts_provider: Option<Arc<dyn TtsProvider>> = if config.qq_bot.enabled
            && config.qq_bot.self_qq_id != 0
            && !config.tts.api_key.is_empty()
        {
            tracing::info!("正在为 QQ 机器人语音消息初始化 VolcengineTtsProvider");
            Some(Arc::new(VolcengineTtsProvider::new(&config.tts)) as Arc<dyn TtsProvider>)
        } else {
            if config.qq_bot.enabled {
                tracing::warn!("未配置 TTS API 密钥 — 语音消息将不可用");
            }
            None
        };

        let _qq_bot_deps = init_qq_bot(
            &config,
            Arc::clone(&ollama_provider),
            qq_bot_tts_provider,
            &mut background,
            qq_bot_bot_account_repo,
            qq_bot_group_repo,
            qq_bot_group_member_repo,
            qq_bot_group_message_repo,
            qq_bot_group_summary_repo,
            qq_bot_group_memory_repo,
            qq_bot_agent_turn_repo,
            qq_bot_outbox_repo,
            // 画像与用户仓库（可选 — 与现有代码模式相同）
            Some(Arc::clone(&user_repo)
                as Arc<
                    dyn crate::domain::user::user_repository::UserRepository,
                >),
            Some(Arc::clone(&qq_bot_external_user_repo)),
            Some(Arc::clone(&qq_bot_user_profile_repo)),
            // 关系仓库
            Some(Arc::clone(&qq_bot_relationship_repo)),
        )
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "qq_bot 初始化失败 — 将继续运行而不启动它");
            None
        });
    }

    // ── 网页知识摄取 ──────────────────────────────────────────────────
    // 审查服务在 worker 禁用时仍可用于检查；
    // 此时提交发布请求将返回冲突。
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
        token_service: Arc::clone(&auth_graph.token_service),
        risk_repo: Arc::clone(&risk_repo),
    };

    let state = bootstrap::state::build_state(&services);
    #[cfg(feature = "qq_bot")]
    let tts_dir = if config.qq_bot.enabled && !config.tts.api_key.is_empty() {
        Some(std::path::PathBuf::from(&config.qq_bot.tts_output_dir))
    } else {
        None
    };
    #[cfg(not(feature = "qq_bot"))]
    let tts_dir: Option<std::path::PathBuf> = None;
    let app = api::router::build_router_with_origins(state, &config.cors.allowed_origins, tts_dir)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("服务器正在监听 http://{addr}");

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
                tracing::warn!(error = %e, "时钟错误");
                continue;
            }
        };
        match repo.cleanup_expired(now).await {
            Ok(n) if n > 0 => tracing::info!(n, "已清理过期令牌"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "清理失败"),
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
        // 用户明确设置了 sqlx 级别 — 尊重它。
        env_filter
    } else {
        // 追加 sqlx=warn 以默认抑制 sqlx 查询日志。
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
