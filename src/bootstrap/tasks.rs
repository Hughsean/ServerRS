use std::sync::Arc;

use crate::domain::tasks::task_handler::TaskHandler;
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::domain::user::user_repository::UserRepoT;
use crate::infra::tasks::alert_handler::{AlertConfig, AlertHandler};
use crate::infra::tasks::in_memory_task_flow::{
    RetryingTaskPublisher, TaskWorker, new_task_channel,
};
use crate::infra::tasks::logging_handler::LoggingHandler;
use crate::infra::tasks::rate_limit_handler::{RateLimitConfig, RateLimitHandler};

pub struct BackgroundTasks {
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl BackgroundTasks {
    pub fn new() -> Self {
        Self {
            handles: Vec::new(),
        }
    }

    pub fn spawn(&mut self, handle: tokio::task::JoinHandle<()>) {
        self.handles.push(handle);
    }

    pub fn abort_all(self) {
        for handle in self.handles {
            handle.abort();
        }
    }
}

impl Default for BackgroundTasks {
    fn default() -> Self {
        Self::new()
    }
}

/// 任务系统上下文。前半段在业务服务之前构建（无服务依赖），
/// 后半段通过 `post_service_setup()` 在服务就绪后完成。
#[allow(dead_code)]
pub struct TaskContext {
    pub task_publisher: Arc<dyn TaskPublisher>,
    pub background: BackgroundTasks,
    task_worker: Option<TaskWorker>,
    alert_handler: Arc<AlertHandler>,
    rate_limit_handler: Arc<RateLimitHandler>,
}

impl TaskContext {
    /// 构造任务系统前半段：告警、限流、通道、Publisher。
    pub fn new(user_repo: Arc<dyn UserRepoT>) -> Self {
        let mut background = BackgroundTasks::new();

        let alert_handler = Arc::new(AlertHandler::new(AlertConfig::default()));
        let rate_limit_handler =
            Arc::new(RateLimitHandler::new(RateLimitConfig::default(), user_repo));

        let (tp, tw) = new_task_channel(256);
        let _retry_handle = RetryingTaskPublisher::spawn_retry_worker(tp.clone());
        let task_worker = tw
            .with_handler(Arc::new(LoggingHandler))
            .with_handler(Arc::clone(&alert_handler) as Arc<dyn TaskHandler>)
            .with_handler(Arc::clone(&rate_limit_handler) as Arc<dyn TaskHandler>);

        // 有状态处理器的定期清理
        {
            let h = Arc::clone(&alert_handler);
            background.spawn(tokio::spawn(async move {
                let mut i = tokio::time::interval(tokio::time::Duration::from_secs(300));
                loop {
                    i.tick().await;
                    h.cleanup().await;
                }
            }));
        }
        {
            let h = Arc::clone(&rate_limit_handler);
            background.spawn(tokio::spawn(async move {
                let mut i = tokio::time::interval(tokio::time::Duration::from_secs(120));
                loop {
                    i.tick().await;
                    h.cleanup().await;
                }
            }));
        }

        let task_publisher: Arc<dyn TaskPublisher> = Arc::new(tp);

        Self {
            task_publisher,
            background,
            task_worker: Some(task_worker),
            alert_handler,
            rate_limit_handler,
        }
    }

    /// 后半段注册：在业务服务就绪后调用。注册依赖服务的 handler 并启动 worker。
    pub fn start_service_handlers(
        &mut self,
        risk_audit_worker: Arc<dyn TaskHandler>,
        summary_refresh_handler: Arc<dyn TaskHandler>,
    ) {
        if let Some(worker) = self.task_worker.take() {
            self.background.spawn(tokio::spawn(
                worker
                    .with_handler(risk_audit_worker)
                    .with_handler(summary_refresh_handler)
                    .run(),
            ));
        }
    }
}
