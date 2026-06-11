mod api;
mod application;
mod domain;
mod infrastructure;
mod shared;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use application::agent::agent_runtime::{AgentRuntime, AgentTool};
use application::agent::agent_context::{AgentContextBuilder, SummaryRepository};
use application::auth::auth_service::AuthService;
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
use application::session::conversation_orchestrator::ConversationOrchestrator;
use application::session::risk_detection_service::RiskDetectionService;
use application::session::session_manager::SessionManager;
use application::session::session_service::SessionService;
use application::session::tool_calling::{ToolCallService, ToolRegistry};
use application::storage::object_service::ObjectService;
use application::user::user_service::UserService;
use domain::agent::AgentEventRepository;
use domain::auth::password_service::PasswordService;
use domain::auth::refresh_token_revocation_repository::RefreshTokenRevocationRepository;
use domain::auth::token_service::TokenService;
use domain::community::CommunityRepository;
use domain::conversation::conversation_repository::ConversationRepository;
use domain::depression::DepressionRepository;
use domain::diary::DiaryRepository;
use domain::like::ContentLikeRepository;
use domain::llm::tools::LlmTool;
use domain::llm::{EmbeddingProvider, LlmClient, LlmProvider, PromptProvider};
use domain::memory::{MemoryRepository, NewSummary};
use domain::music::MusicRepository;
use domain::psychology::PsychologyRepository;
use domain::rag::RAGRepository;
use domain::risk::risk_detector::RiskDetector;
use domain::risk::risk_repository::RiskRepository;
use domain::storage::ObjectStorage;
use domain::tasks::task_handler::TaskHandler;
use domain::tasks::task_publisher::TaskPublisher;
use domain::user::user_profile_repository::UserProfileRepository;
use domain::user::user_repository::UserRepository;
use infrastructure::auth::bcrypt_password_hasher::BcryptPasswordHasher;
use infrastructure::auth::jwt_token_service::JwtTokenService;
use infrastructure::detector::rule_based_detector::RuleBasedRiskDetector;
use infrastructure::llm::ollama_client::OllamaClient;
use infrastructure::llm::ollama_provider::OllamaProvider;
use infrastructure::llm::plugins::{
    GetNewsTool, GetTimeTool, GetWeatherTool, HandleExitIntentTool,
};
use infrastructure::llm::prompt_provider::PromptProvider as InfraPromptProvider;
use infrastructure::persistence::implementations::seaorm_agent_repository::SeaOrmAgentEventRepository;
use infrastructure::persistence::implementations::seaorm_community_repository::SeaOrmCommunityRepository;
use infrastructure::persistence::implementations::seaorm_conversation_repository::SeaOrmConversationRepository;
use infrastructure::persistence::implementations::seaorm_depression_repository::SeaOrmDepressionRepository;
use infrastructure::persistence::implementations::seaorm_diary_repository::SeaOrmDiaryRepository;
use infrastructure::persistence::implementations::seaorm_like_repository::SeaOrmLikeRepository;
use infrastructure::persistence::implementations::seaorm_music_repository::SeaOrmMusicRepository;
use infrastructure::persistence::implementations::seaorm_psychology_repository::SeaOrmPsychologyRepository;
use infrastructure::persistence::implementations::seaorm_refresh_token_store::SeaOrmRefreshTokenStore;
use infrastructure::persistence::implementations::seaorm_risk_repository::SeaOrmRiskRepository;
use infrastructure::persistence::implementations::seaorm_user_profile_repository::SeaOrmUserProfileRepository;
use infrastructure::persistence::implementations::seaorm_user_repository::SeaOrmUserRepository;
use infrastructure::persistence::seaorm_db::init_db;
use infrastructure::storage::local_storage::LocalObjectStorage;
use infrastructure::tasks::alert_handler::{AlertConfig, AlertHandler};
use infrastructure::tasks::in_memory_task_flow::new_task_channel;
use infrastructure::tasks::logging_handler::LoggingHandler;
use infrastructure::tasks::rate_limit_handler::{RateLimitConfig, RateLimitHandler};

use shared::config::AppConfig;
use shared::error::AppError;
use tracing::info;

// ── Stub repositories for traits that lack a SeaORM implementation ────────
// These will be replaced once the corresponding infrastructure modules exist.

struct StubRAGRepository;
#[async_trait::async_trait]
impl RAGRepository for StubRAGRepository {
    async fn save_document(&self, _doc: domain::rag::NewDocument) -> Result<domain::rag::KnowledgeDocument, AppError> {
        Err(AppError::Internal("StubRAGRepository — not implemented".into()))
    }
    async fn find_document_by_source(&self, _source_type: &str, _source_id: Option<u64>) -> Result<Option<domain::rag::KnowledgeDocument>, AppError> {
        Ok(None)
    }
    async fn list_documents_by_source_type(&self, _source_type: &str) -> Result<Vec<domain::rag::KnowledgeDocument>, AppError> {
        Ok(vec![])
    }
    async fn save_chunks(&self, _chunks: &[domain::rag::NewChunk]) -> Result<Vec<domain::rag::KnowledgeChunk>, AppError> {
        Err(AppError::Internal("StubRAGRepository — not implemented".into()))
    }
    async fn find_chunks_by_document(&self, _document_id: u64) -> Result<Vec<domain::rag::KnowledgeChunk>, AppError> {
        Ok(vec![])
    }
    async fn save_embedding(&self, _emb: domain::rag::NewEmbedding) -> Result<domain::rag::KnowledgeEmbedding, AppError> {
        Err(AppError::Internal("StubRAGRepository — not implemented".into()))
    }
    async fn find_embedding_by_chunk(&self, _chunk_id: u64) -> Result<Option<domain::rag::KnowledgeEmbedding>, AppError> {
        Ok(None)
    }
    async fn search_by_keyword(&self, _query: &str, _top_k: u64) -> Result<Vec<(domain::rag::KnowledgeChunk, f64)>, AppError> {
        Ok(vec![])
    }
    async fn delete_document(&self, _document_id: u64) -> Result<(), AppError> {
        Err(AppError::Internal("StubRAGRepository — not implemented".into()))
    }
    async fn list_chunks_with_embeddings(&self) -> Result<Vec<(domain::rag::KnowledgeChunk, domain::rag::KnowledgeEmbedding)>, AppError> {
        Ok(vec![])
    }
}

struct StubMemoryRepository;
#[async_trait::async_trait]
impl MemoryRepository for StubMemoryRepository {
    async fn save_memory(&self, _memory: domain::memory::NewMemory) -> Result<domain::memory::UserMemory, AppError> {
        Err(AppError::Internal("StubMemoryRepository — not implemented".into()))
    }
    async fn find_by_id(&self, _memory_id: u64) -> Result<Option<domain::memory::UserMemory>, AppError> {
        Ok(None)
    }
    async fn find_by_user_id(&self, _user_id: u64, _status: Option<i8>) -> Result<Vec<domain::memory::UserMemory>, AppError> {
        Ok(vec![])
    }
    async fn search_by_user(&self, _user_id: u64, _query: &str, _top_k: u32) -> Result<Vec<domain::memory::UserMemory>, AppError> {
        Ok(vec![])
    }
    async fn update_memory(&self, _memory_id: u64, _content: Option<String>, _confidence: Option<f64>) -> Result<domain::memory::UserMemory, AppError> {
        Err(AppError::Internal("StubMemoryRepository — not implemented".into()))
    }
    async fn disable_memory(&self, _memory_id: u64) -> Result<(), AppError> {
        Err(AppError::Internal("StubMemoryRepository — not implemented".into()))
    }
    async fn delete_memory(&self, _memory_id: u64) -> Result<bool, AppError> {
        Ok(true)
    }
    async fn find_memories_by_conversation(&self, _conversation_id: u64) -> Result<Vec<domain::memory::UserMemory>, AppError> {
        Ok(vec![])
    }
}

struct StubSummaryRepository;
#[async_trait::async_trait]
impl SummaryRepository for StubSummaryRepository {
    async fn find_latest_by_conversation(&self, _conversation_id: u64) -> Result<Option<String>, AppError> {
        Ok(None)
    }
    async fn save_summary(&self, _summary: NewSummary) -> Result<(), AppError> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    init_tracing();
    if let Err(err) = run().await {
        tracing::error!(error = %err, "server stopped with error");
    }
}

async fn run() -> Result<(), std::io::Error> {
    let config = AppConfig::load();
    let db = init_db(&config.database.url).await.expect("db init");

    // ── Repositories ──
    let user_repo: Arc<dyn UserRepository> = Arc::new(SeaOrmUserRepository::new(db.clone()));
    let profile_repo: Arc<dyn UserProfileRepository> =
        Arc::new(SeaOrmUserProfileRepository::new(db.clone()));
    let conv_repo: Arc<dyn ConversationRepository> =
        Arc::new(SeaOrmConversationRepository::new(db.clone()));
    let risk_repo: Arc<dyn RiskRepository> = Arc::new(SeaOrmRiskRepository::new(db.clone()));
    let psychology_repo: Arc<dyn PsychologyRepository> =
        Arc::new(SeaOrmPsychologyRepository::new(db.clone()));
    let depression_repo: Arc<dyn DepressionRepository> =
        Arc::new(SeaOrmDepressionRepository::new(db.clone()));
    let diary_repo: Arc<dyn DiaryRepository> =
        Arc::new(SeaOrmDiaryRepository::new(db.clone()));
    let music_repo: Arc<dyn MusicRepository> =
        Arc::new(SeaOrmMusicRepository::new(db.clone()));
    let community_repo: Arc<dyn CommunityRepository> =
        Arc::new(SeaOrmCommunityRepository::new(db.clone()));
    let like_repo: Arc<dyn ContentLikeRepository> =
        Arc::new(SeaOrmLikeRepository::new(db.clone()));
    let agent_event_repo: Arc<dyn AgentEventRepository> =
        Arc::new(SeaOrmAgentEventRepository::new(db.clone()));

    // ── RAG / memory stub repos (awaiting SeaORM implementations) ──
    let rag_repo: Arc<dyn RAGRepository> = Arc::new(StubRAGRepository);
    let memory_repo: Arc<dyn MemoryRepository> = Arc::new(StubMemoryRepository);
    let summary_repo: Arc<dyn SummaryRepository> = Arc::new(StubSummaryRepository);

    // ── Tasks ──
    let alert_handler = Arc::new(AlertHandler::new(AlertConfig::default()));
    let rate_limit_handler = Arc::new(RateLimitHandler::new(
        RateLimitConfig::default(),
        Arc::clone(&user_repo),
    ));

    let (tp, tw) = new_task_channel(256);
    let tw_handle = tokio::spawn(
        tw.with_handler(Arc::new(LoggingHandler))
            .with_handler(Arc::clone(&alert_handler) as Arc<dyn TaskHandler>)
            .with_handler(Arc::clone(&rate_limit_handler) as Arc<dyn TaskHandler>)
            .run(),
    );
    let task_publisher: Arc<dyn TaskPublisher> = Arc::new(tp);

    // Periodic cleanup for stateful handlers
    let alert_cleanup = {
        let h = Arc::clone(&alert_handler);
        tokio::spawn(async move {
            let mut i = tokio::time::interval(tokio::time::Duration::from_secs(300));
            loop {
                i.tick().await;
                h.cleanup().await;
            }
        })
    };
    let rl_cleanup = {
        let h = Arc::clone(&rate_limit_handler);
        tokio::spawn(async move {
            let mut i = tokio::time::interval(tokio::time::Duration::from_secs(120));
            loop {
                i.tick().await;
                h.cleanup().await;
            }
        })
    };

    // ── Auth infra ──
    let password_service: Arc<dyn PasswordService> = Arc::new(BcryptPasswordHasher::default());
    let revoke_repo: Arc<SeaOrmRefreshTokenStore> = Arc::new(SeaOrmRefreshTokenStore::new(db.clone()));
    let jwt: Arc<JwtTokenService> = Arc::new(JwtTokenService::new(
        &config.jwt.secret,
        config.jwt.access_ttl_secs,
    ));

    // ── LLM (infrastructure → domain trait) ──
    // Legacy LlmClient (used by ConversationOrchestrator, ToolCallService, DiaryService)
    let ollama_client: Arc<dyn LlmClient> = Arc::new(OllamaClient::new(
        config.ollama.base_url.clone(),
        config.ollama.model.clone(),
        config.ollama.temperature,
        config.ollama.top_p,
    ));
    // New LlmProvider + EmbeddingProvider (used by AgentRuntime, MemoryExtractor, RAG services)
    let ollama_provider: Arc<dyn LlmProvider> = Arc::new(OllamaProvider::new(
        config.ollama.base_url.clone(),
        config.ollama.model.clone(),
        config.ollama.temperature,
        config.ollama.top_p,
    ));
    let embedding_provider: Arc<dyn EmbeddingProvider> = Arc::new(OllamaProvider::new(
        config.ollama.base_url.clone(),
        config.ollama.model.clone(),
        config.ollama.temperature,
        config.ollama.top_p,
    ));
    let prompt_provider: Arc<dyn PromptProvider> = Arc::new(InfraPromptProvider::new(None));

    // ── Risk detector ──
    let risk_detector: Arc<dyn RiskDetector> = Arc::new(RuleBasedRiskDetector::new());

    // ── Services ──
    let auth: Arc<AuthService> = Arc::new(AuthService::new(
        Arc::clone(&user_repo),
        Arc::clone(&password_service) as Arc<dyn PasswordService>,
        Arc::clone(&jwt) as Arc<dyn TokenService>,
        Arc::clone(&revoke_repo) as Arc<dyn application::auth::auth_service::RefreshTokenStore>,
        Arc::clone(&task_publisher),
        application::auth::auth_service::AuthConfig {
            max_attempts: config.auth.max_login_attempts,
            lockout_secs: config.auth.lockout_duration_secs,
        },
    ));
    let user: Arc<UserService> = Arc::new(UserService::new(
        Arc::clone(&user_repo),
        Arc::clone(&profile_repo),
    ));
    let query: Arc<SessionService> = Arc::new(SessionService::new(
        Arc::clone(&conv_repo),
        Arc::clone(&risk_repo),
    ));
    let risk_detect: Arc<RiskDetectionService> = Arc::new(RiskDetectionService::new(
        Arc::clone(&risk_repo),
        Arc::clone(&task_publisher),
        Arc::clone(&risk_detector),
    ));

    let orchestrator: Arc<ConversationOrchestrator> = Arc::new(ConversationOrchestrator::new(
        Arc::clone(&task_publisher),
        Arc::clone(&ollama_client),
        Arc::clone(&prompt_provider),
        Arc::clone(&conv_repo) as Arc<dyn ConversationRepository>,
        Arc::clone(&profile_repo),
    ));

    // ── Plugins config ──
    let news_rss_url = config.plugins.news.rss_url.clone();

    let tool_registry = ToolRegistry::new(vec![
        Arc::new(GetTimeTool::new()) as Arc<dyn LlmTool>,
        Arc::new(GetWeatherTool::new()) as Arc<dyn LlmTool>,
        Arc::new(GetNewsTool::new(None, news_rss_url)) as Arc<dyn LlmTool>,
        Arc::new(HandleExitIntentTool::new()) as Arc<dyn LlmTool>,
    ]);
    let tool_service: Arc<ToolCallService> = Arc::new(ToolCallService::new(
        Arc::clone(&ollama_client),
        tool_registry,
        3,
        std::time::Duration::from_millis(5000),
    ));

    let session: Arc<SessionManager> = Arc::new(SessionManager::new(
        Arc::clone(&task_publisher),
        Arc::clone(&risk_detect),
        Arc::clone(&orchestrator),
        Arc::clone(&tool_service),
        config.session.timeout_seconds,
    ));
    let sess_cleanup = {
        let s = Arc::clone(&session);
        tokio::spawn(async move {
            let mut i = tokio::time::interval(tokio::time::Duration::from_secs(60));
            loop {
                i.tick().await;
                s.cleanup().await;
            }
        })
    };

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
    let community: Arc<CommunityService> = Arc::new(CommunityService::new(Arc::clone(&community_repo)));
    let objects: Arc<ObjectService> = Arc::new(ObjectService::new(
        Arc::clone(&local_storage),
        config.storage.clone(),
    ));

    // ── RAG services (new, backed by LlmProvider) ──
    let retrieval: Arc<RetrievalService> = Arc::new(RetrievalService::new(
        Arc::clone(&rag_repo),
        Some(Arc::clone(&embedding_provider)),
    ));
    let chunking = ChunkingService::new();
    let ingestion: Arc<IngestionService> = Arc::new(IngestionService::new(
        Arc::clone(&rag_repo),
        chunking,
        Some(Arc::clone(&embedding_provider)),
    ));

    // ── Memory services (new, backed by LlmProvider) ──
    let memory_extractor: Arc<MemoryExtractor> = Arc::new(MemoryExtractor::new(Arc::clone(&ollama_provider)));
    let memory_svc: Arc<MemoryService> = Arc::new(MemoryService::new(
        Arc::clone(&memory_repo),
        memory_extractor,
    ));

    // ── Agent Runtime ──
    let context_builder: Arc<AgentContextBuilder> = Arc::new(AgentContextBuilder::new(
        Arc::clone(&memory_repo),
        Arc::clone(&rag_repo),
        Arc::clone(&summary_repo),
        Arc::clone(&conv_repo),
        Arc::clone(&profile_repo),
    ));

    let agent_tools: Vec<Arc<dyn AgentTool>> = Vec::new(); // tools registered as they are implemented

    let agent_runtime: Arc<AgentRuntime> = Arc::new(AgentRuntime::new(
        Arc::clone(&ollama_provider),
        Arc::clone(&rag_repo),
        Arc::clone(&memory_repo),
        Arc::clone(&risk_detector),
        Arc::clone(&risk_repo),
        Arc::clone(&agent_event_repo),
        Arc::clone(&conv_repo),
        Arc::clone(&profile_repo),
        context_builder,
        agent_tools,
        10, // max_tool_depth
    ));

    // ── API ──
    let state = api::ApiState {
        auth,
        user,
        session,
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
    };
    let app = api::router::build_router(state);

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("server listening on http://{addr}");

    let r = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;
    tw_handle.abort();
    sess_cleanup.abort();
    alert_cleanup.abort();
    rl_cleanup.abort();
    r
}

async fn periodic_revocation(repo: Arc<dyn RefreshTokenRevocationRepository>) {
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

fn init_tracing() {
    let f = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(f)
        .compact()
        .init();
}
