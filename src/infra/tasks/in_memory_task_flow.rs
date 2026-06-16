use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::domain::tasks::task_event::TaskEvent;
use crate::domain::tasks::task_handler::TaskHandler;
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::shared::error::AppError;

// ── Resilient publisher ──

#[derive(Clone)]
pub struct ResilientTaskPublisher {
    sender: mpsc::UnboundedSender<TaskEvent>,
}

impl ResilientTaskPublisher {
    fn new(sender: mpsc::UnboundedSender<TaskEvent>) -> Self {
        Self { sender }
    }
}

#[async_trait]
impl TaskPublisher for ResilientTaskPublisher {
    async fn publish(&self, event: TaskEvent) -> Result<(), AppError> {
        self.sender
            .send(event)
            .map_err(|_| AppError::Infrastructure("task channel closed".into()))
    }
}

// ── Worker ──

pub struct TaskWorker {
    receiver: mpsc::UnboundedReceiver<TaskEvent>,
    handlers: Vec<Arc<dyn TaskHandler>>,
}

/// 创建通道对。Worker 启动时**没有处理器** —
/// 使用 `TaskWorker::with_handler` 注入 `LoggingHandler` 或自定义处理器。
pub fn new_task_channel(_buffer: usize) -> (ResilientTaskPublisher, TaskWorker) {
    let (tx, rx) = mpsc::unbounded_channel();
    let worker = TaskWorker {
        receiver: rx,
        handlers: Vec::new(),
    };
    (ResilientTaskPublisher::new(tx), worker)
}

impl TaskWorker {
    pub fn with_handler(mut self, handler: Arc<dyn TaskHandler>) -> Self {
        self.handlers.push(handler);
        self
    }

    pub async fn run(mut self) {
        info!(
            handlers = ?self.handlers.iter().map(|h| h.name()).collect::<Vec<_>>(),
            "task worker starting"
        );
        while let Some(event) = self.receiver.recv().await {
            for h in &self.handlers {
                debug!(handler = h.name(), event = ?event, "dispatching");
                h.handle(&event).await;
            }
        }
        info!("task worker stopped");
    }
}
