use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use domain::auth::refresh_token_store::RefreshTokenStore;
use server_rs::{api, bootstrap, domain, shared};

use shared::config::AppConfig;
use tracing::info;

#[tokio::main]
async fn main() {
    let config = AppConfig::load();
    init_tracing(&config.logging.level);
    if let Err(err) = run(config).await {
        tracing::error!(error = %err, "服务器运行出错");
    }
}

async fn run(config: AppConfig) -> Result<(), std::io::Error> {
    // ── 基础设施（SSH 隧道、DB、LLM）──
    let infra = bootstrap::infra::InfraContext::new(&config).await?;

    // ── 仓库 ──
    let repos = bootstrap::repos::build_repos(
        &infra.db,
        &config.qdrant.memory_collection,
        &config.qdrant.summary_collection,
    );

    // ── 任务系统 ──
    let mut tasks = bootstrap::tasks::TaskContext::new(Arc::clone(&repos.user_repo));

    // ── 认证基础设施 ──
    let auth_graph =
        bootstrap::auth::build_auth(&infra.db, &config.jwt, &config.auth, &repos.user_repo, &tasks.task_publisher);
    tasks.background.spawn({
        let store = Arc::clone(&auth_graph.refresh_token_store);
        tokio::spawn(periodic_revocation(store))
    });

    // ── 向量/RAG 装配（Embedding → Qdrant → VectorIndex）──
    let vector = bootstrap::vector::VectorContext::new(&config, &infra, &repos).await?;
    vector.ensure_collections().await?;

    // ── 业务服务 ──
    let services = bootstrap::state::ServiceGraph::build(
        &config, &infra, &repos, &vector, &mut tasks,
    ).await?;

    // ── API ──

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
    tasks.background.abort_all();

    // 关闭 SSH 隧道
    if let Some(manager) = infra._ssh_manager {
        manager.shutdown().await;
    }

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
