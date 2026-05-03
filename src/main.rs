mod api;
mod application;
mod domain;
mod infrastructure;
mod shared;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use application::auth::auth_service::AuthService;
use application::session::conversation_orchestrator::ConversationOrchestrator;
use application::session::risk_detection_service::RiskDetectionService;
use application::session::session_manager::SessionManager;
use application::session::session_service::SessionService;
use application::session::tool_calling::{ToolCallService, ToolRegistry};
use application::user::user_service::UserService;
use domain::auth::password_service::PasswordService;
use domain::auth::refresh_token_revocation_repository::RefreshTokenRevocationRepository;
use domain::auth::refresh_token_service::RefreshTokenService;
use domain::auth::token_service::TokenService;
use domain::conversation::conversation_repository::ConversationRepository;
use domain::llm::tools::LlmTool;
use domain::llm::{LlmClient, PromptProvider};
use domain::risk::risk_detector::RiskDetector;
use domain::risk::risk_repository::RiskRepository;
use domain::tasks::task_handler::TaskHandler;
use domain::tasks::task_publisher::TaskPublisher;
use domain::user::user_profile_repository::UserProfileRepository;
use domain::user::user_repository::UserRepository;
use infrastructure::auth::bcrypt_password_hasher::BcryptPasswordHasher;
use infrastructure::auth::in_memory_refresh_token_revocation_repository::InMemoryRefreshTokenRevocationRepository;
use infrastructure::auth::jwt_token_service::JwtTokenService;
use infrastructure::detector::rule_based_detector::RuleBasedRiskDetector;
use infrastructure::llm::ollama_client::OllamaClient;
use infrastructure::llm::plugins::{GetTimeTool, HandleExitIntentTool};
use infrastructure::llm::prompt_provider::PromptProvider as InfraPromptProvider;
use infrastructure::persistence::database::init_db;
use infrastructure::persistence::seaorm_conversation_repository::SeaOrmConversationRepository;
use infrastructure::persistence::seaorm_risk_repository::SeaOrmRiskRepository;
use infrastructure::persistence::seaorm_user_profile_repository::SeaOrmUserProfileRepository;
use infrastructure::persistence::seaorm_user_repository::SeaOrmUserRepository;
use infrastructure::tasks::alert_handler::{AlertConfig, AlertHandler};
use infrastructure::tasks::in_memory_task_flow::new_task_channel;
use infrastructure::tasks::logging_handler::LoggingHandler;
use infrastructure::tasks::rate_limit_handler::{RateLimitConfig, RateLimitHandler};

use shared::config::AppConfig;
use tracing::info;

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
    let revoke_repo: Arc<dyn RefreshTokenRevocationRepository> =
        Arc::new(InMemoryRefreshTokenRevocationRepository::new());
    let revoke_cleanup = tokio::spawn(periodic_revocation(Arc::clone(&revoke_repo)));
    let jwt: Arc<JwtTokenService> = Arc::new(JwtTokenService::new(
        &config.jwt.secret,
        config.jwt.expiration_secs,
    ));

    // ── LLM (infrastructure → domain trait) ──
    let ollama: Arc<dyn LlmClient> = Arc::new(OllamaClient::new(
        config
            .ollama
            .as_ref()
            .map_or("http://10.13.19.91:11434/v1".into(), |o| o.base_url.clone()),
        config
            .ollama
            .as_ref()
            .map_or("qwen2.5:14b".into(), |o| o.model.clone()),
        config.ollama.as_ref().map_or(0.5, |o| o.temperature),
        config.ollama.as_ref().map_or(0.9, |o| o.top_p),
    ));
    let prompt_provider: Arc<dyn PromptProvider> = Arc::new(InfraPromptProvider::new(None));

    // ── Risk detector ──
    let risk_detector: Arc<dyn RiskDetector> = Arc::new(RuleBasedRiskDetector::new());

    // ── Services ──
    let auth: Arc<AuthService> = Arc::new(AuthService::new(
        Arc::clone(&user_repo),
        Arc::clone(&password_service) as Arc<dyn PasswordService>,
        Arc::clone(&jwt) as Arc<dyn TokenService>,
        Arc::clone(&jwt) as Arc<dyn RefreshTokenService>,
        Arc::clone(&revoke_repo),
        Arc::clone(&task_publisher),
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
        Arc::clone(&ollama),
        Arc::clone(&prompt_provider),
        Arc::clone(&conv_repo) as Arc<dyn ConversationRepository>,
        Arc::clone(&profile_repo),
    ));

    let tool_registry = ToolRegistry::new(vec![
        Arc::new(GetTimeTool::new()) as Arc<dyn LlmTool>,
        Arc::new(HandleExitIntentTool::new()) as Arc<dyn LlmTool>,
    ]);
    let tool_service: Arc<ToolCallService> = Arc::new(ToolCallService::new(
        Arc::clone(&ollama),
        tool_registry,
        3,
        std::time::Duration::from_millis(5000),
    ));

    let session: Arc<SessionManager> = Arc::new(SessionManager::new(
        Arc::clone(&task_publisher),
        Arc::clone(&risk_detect),
        Arc::clone(&orchestrator),
        Arc::clone(&tool_service),
        config.session.as_ref().map_or(120, |s| s.timeout_seconds),
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

    // ── API ──
    let state = api::ApiState {
        auth,
        user,
        session,
        query,
    };
    let app = api::router::build_router(state);

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("server listening on http://{addr}");

    let r = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;
    tw_handle.abort();
    revoke_cleanup.abort();
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
