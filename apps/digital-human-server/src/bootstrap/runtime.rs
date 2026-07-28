use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::api::router;
use crate::bootstrap::auth::build_auth;
use crate::bootstrap::infra::InfraContext;
use crate::bootstrap::repos::build_repos;
use crate::bootstrap::state::{ServiceGraph, build_state};
use crate::bootstrap::tasks::{BackgroundTasks, TaskContext};
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
        &config.vector_store.memory_index_name,
        &config.vector_store.summary_index_name,
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
    vector.ensure_indexes().await?;
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
    if let Some(tts) = services.tts.clone() {
        tasks.background.spawn(tokio::spawn(periodic_tts_cleanup(
            tts,
            config.tts.audio_retention_secs,
            config.tts.audio_cleanup_interval_secs,
            shutdown_token.clone(),
        )));
    }
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
    let mut task_guard =
        RuntimeTaskGuard::new(shutdown_token.clone(), dispatcher_handle, tasks.background);
    let state = build_state(&services);

    let app = router::build_router_with_origins(state, &config.cors.allowed_origins)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("服务器正在监听 http://{addr}");

    let shutdown_watcher = tokio::spawn({
        let shutdown_token = shutdown_token.clone();
        async move {
            shutdown_signal().await;
            shutdown_token.cancel();
        }
    });
    let server_shutdown = {
        let shutdown_token = shutdown_token.clone();
        async move {
            shutdown_token.cancelled().await;
        }
    };

    let r = axum::serve(listener, app)
        .with_graceful_shutdown(server_shutdown)
        .await;

    shutdown_watcher.abort();

    // 1. 通知所有监听 shutdown token 的后台任务停止；如果 server 因错误返回，也确保 token 被取消。
    shutdown_token.cancel();

    // 2. 等待 dispatcher 自身执行优雅 drain。
    if let Some(handle) = task_guard.take_dispatcher_handle() {
        if let Err(e) = handle.await {
            tracing::error!(error = %e, "dispatcher task join error");
        }
    }

    // 3. 通用后台任务没有统一取消协议，直接 abort。
    task_guard.abort_background();
    task_guard.disarm();

    // 4. 关闭 SSH 隧道。
    if let Some(manager) = infra._ssh_manager {
        manager.shutdown().await;
    }

    r
}

struct RuntimeTaskGuard {
    shutdown_token: CancellationToken,
    dispatcher_handle: Option<tokio::task::JoinHandle<()>>,
    background: Option<BackgroundTasks>,
    armed: bool,
}

impl RuntimeTaskGuard {
    fn new(
        shutdown_token: CancellationToken,
        dispatcher_handle: Option<tokio::task::JoinHandle<()>>,
        background: BackgroundTasks,
    ) -> Self {
        Self {
            shutdown_token,
            dispatcher_handle,
            background: Some(background),
            armed: true,
        }
    }

    fn take_dispatcher_handle(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        self.dispatcher_handle.take()
    }

    fn abort_background(&mut self) {
        if let Some(background) = self.background.take() {
            background.abort_all();
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RuntimeTaskGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        self.shutdown_token.cancel();
        if let Some(handle) = self.dispatcher_handle.take() {
            handle.abort();
        }
        if let Some(background) = self.background.take() {
            background.abort_all();
        }
    }
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

/// 定期清理数字人独立语音目录中过期的 UUID 音频文件。
async fn periodic_tts_cleanup(
    tts: Arc<crate::app::tts::tts_service::TtsService>,
    retention_secs: u64,
    interval_secs: u64,
    shutdown_token: CancellationToken,
) {
    let retention = tokio::time::Duration::from_secs(retention_secs);
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));
    loop {
        tokio::select! {
            _ = shutdown_token.cancelled() => break,
            _ = interval.tick() => match tts.cleanup_expired(retention).await {
                Ok(removed) if removed > 0 => tracing::info!(removed, "已清理过期数字人语音文件"),
                Ok(_) => {}
                Err(error) => tracing::warn!(error = %error, "清理数字人语音文件失败"),
            },
        }
    }
}

/// 监听平台终止信号，用于优雅关闭。
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
    #[cfg(unix)]
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }

    #[cfg(windows)]
    {
        let registration = match windows_console_shutdown::Registration::register() {
            Ok(registration) => Some(registration),
            Err(error) => {
                tracing::warn!(error = %error, "注册 Windows 控制台关闭通知失败，将仅响应 Ctrl+C");
                None
            }
        };
        let console_close = async {
            if registration.is_some() {
                windows_console_shutdown::wait_for_close_event().await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! { _ = ctrl_c => {}, _ = console_close => {} }
    }

    #[cfg(all(not(unix), not(windows)))]
    ctrl_c.await;
}

/// Windows 关闭控制台会产生 CTRL_CLOSE_EVENT，而 Tokio 的 `ctrl_c` 不会覆盖它。
/// 控制台回调受系统的短时限约束，因此只设置原子标记；异步清理由运行时任务完成。
#[cfg(windows)]
mod windows_console_shutdown {
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::System::Console::{
        CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
    };

    static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

    pub struct Registration;

    impl Registration {
        pub fn register() -> io::Result<Self> {
            CLOSE_REQUESTED.store(false, Ordering::Release);
            // SAFETY: handler 是静态函数，进程存活期间地址有效；Drop 中会解除注册。
            let registered = unsafe { SetConsoleCtrlHandler(Some(console_control_handler), 1) };
            if registered == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self)
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            // SAFETY: 与注册时传入同一个静态回调；注销失败时进程即将退出，不影响资源回收。
            unsafe {
                SetConsoleCtrlHandler(Some(console_control_handler), 0);
            }
        }
    }

    pub async fn wait_for_close_event() {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(25));
        loop {
            interval.tick().await;
            if CLOSE_REQUESTED.swap(false, Ordering::AcqRel) {
                return;
            }
        }
    }

    unsafe extern "system" fn console_control_handler(control_type: u32) -> i32 {
        match control_type {
            CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
                CLOSE_REQUESTED.store(true, Ordering::Release);
                1
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future;
    use std::time::Duration;

    use tokio::sync::oneshot;

    use super::*;
    use crate::bootstrap::tasks::BackgroundTasks;

    struct AbortNotify(Option<oneshot::Sender<()>>);

    impl AbortNotify {
        fn new(tx: oneshot::Sender<()>) -> Self {
            Self(Some(tx))
        }
    }

    impl Drop for AbortNotify {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }

    #[tokio::test]
    async fn runtime_task_guard_aborts_background_and_dispatcher_on_drop() {
        let shutdown_token = CancellationToken::new();
        let (dispatcher_tx, dispatcher_rx) = oneshot::channel();
        let dispatcher_handle = tokio::spawn(async move {
            let _notify = AbortNotify::new(dispatcher_tx);
            future::pending::<()>().await;
        });

        let (background_tx, background_rx) = oneshot::channel();
        let mut background = BackgroundTasks::new();
        background.spawn(tokio::spawn(async move {
            let _notify = AbortNotify::new(background_tx);
            future::pending::<()>().await;
        }));
        tokio::task::yield_now().await;

        let guard =
            RuntimeTaskGuard::new(shutdown_token.clone(), Some(dispatcher_handle), background);
        drop(guard);

        assert!(shutdown_token.is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), dispatcher_rx)
            .await
            .expect("dispatcher task 应该被 abort")
            .expect("dispatcher abort 通知应该送达");
        tokio::time::timeout(Duration::from_secs(1), background_rx)
            .await
            .expect("background task 应该被 abort")
            .expect("background abort 通知应该送达");
    }
}
