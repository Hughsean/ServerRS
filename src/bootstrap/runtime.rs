use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::api::router;
use crate::bootstrap::auth::build_auth;
use crate::bootstrap::infra::InfraContext;
use crate::bootstrap::repos::build_repos;
use crate::bootstrap::state::{ServiceGraph, build_state};
use crate::bootstrap::tasks::TaskContext;
use crate::bootstrap::vector::VectorContext;
use crate::domain::auth::refresh_token_store::RefreshTokenStoreT;
use crate::shared::config::AppConfig;

/// 顶层启动编排。按 6 阶段顺序执行：
///   基础设施 → 仓库 → 任务系统 → 向量/RAG → 业务服务 → HTTP 服务
pub async fn run(config: AppConfig) -> Result<(), std::io::Error> {
    let shutdown_token = CancellationToken::new();
    // 阶段 1: 基础设施
    let infra = InfraContext::new(&config).await?;
    // 阶段 2: 仓库
    let repos = build_repos(
        &infra.db,
        &config.qdrant.memory_collection,
        &config.qdrant.summary_collection,
    );
    // 阶段 3: 任务系统
    let mut tasks = TaskContext::new(Arc::clone(&repos.user_repo));
    // 认证（在向量之前，因为任务清理需要它）
    let auth_graph = build_auth(
        &infra.db,
        &config.jwt,
        &config.auth,
        &repos.user_repo,
        &tasks.task_publisher,
    );
    tasks.background.spawn({
        let store = Arc::clone(&auth_graph.refresh_token_store);
        tokio::spawn(periodic_revocation(store))
    });
    // 阶段 4: 向量/RAG
    let vector = VectorContext::new(&config, &infra, &repos).await?;
    vector.ensure_collections().await?;
    // 阶段 5: 业务服务
    let services = ServiceGraph::build(
        &config,
        &infra,
        &repos,
        &vector,
        &mut tasks,
        &auth_graph,
        shutdown_token.clone(),
    )
    .await?;
    // 阶段 6: HTTP 服务
    serve(&config, services, tasks, infra, shutdown_token).await
}

/// HTTP 服务启动 + 优雅关闭 + 资源回收。
async fn serve(
    config: &AppConfig,
    mut services: ServiceGraph,
    tasks: TaskContext,
    infra: InfraContext,
    shutdown_token: CancellationToken,
) -> Result<(), std::io::Error> {
    let dispatcher_handle = services.dispatcher_handle.take();
    let state = build_state(&services);

    #[cfg(feature = "qq_bot")]
    let tts_dir = if config.qq_bot.enabled && !config.tts.api_key.is_empty() {
        Some(std::path::PathBuf::from(&config.qq_bot.tts_output_dir))
    } else {
        None
    };
    #[cfg(not(feature = "qq_bot"))]
    let tts_dir: Option<std::path::PathBuf> = None;

    let app = router::build_router_with_origins(state, &config.cors.allowed_origins, tts_dir)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("服务器正在监听 http://{addr}");

    let r = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;

    // 1. Notify all background tasks to stop
    shutdown_token.cancel();

    // 2. Wait for dispatcher graceful drain (has internal grace timeout + abort_all fallback)
    if let Some(handle) = dispatcher_handle {
        if let Err(e) = handle.await {
            tracing::error!(error = %e, "dispatcher task join error");
        }
    }

    // 3. Abort remaining generic background tasks (periodic cleanup, etc.)
    tasks.background.abort_all();

    // 4. Shut down SSH tunnels
    if let Some(manager) = infra._ssh_manager {
        manager.shutdown().await;
    }

    r
}

/// 定期清理过期的 JWT refresh token。
async fn periodic_revocation(repo: Arc<dyn RefreshTokenStoreT>) {
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

/// 监听 SIGTERM（Unix）和 Ctrl+C（所有平台），用于优雅关闭。
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
