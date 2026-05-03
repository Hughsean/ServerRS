use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::domain::tasks::task_event::TaskEvent;
use crate::domain::tasks::task_handler::TaskHandler;
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::shared::error::AppError;

// ── Resilient publisher ──

#[derive(Clone)]
pub struct ResilientTaskPublisher {
    sender: mpsc::Sender<TaskEvent>,
}

impl ResilientTaskPublisher {
    fn new(sender: mpsc::Sender<TaskEvent>) -> Self {
        Self { sender }
    }
}

#[async_trait]
impl TaskPublisher for ResilientTaskPublisher {
    async fn publish(&self, event: TaskEvent) -> Result<(), AppError> {
        match self.sender.try_send(event) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!("task channel full, event dropped");
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(AppError::Infrastructure("task channel closed".into()))
            }
        }
    }
}

// ── Worker ──

pub struct TaskWorker {
    receiver: mpsc::Receiver<TaskEvent>,
    handlers: Vec<Arc<dyn TaskHandler>>,
}

/// Creates a channel pair. The worker starts with **no handlers** —
/// use `TaskWorker::with_handler` to inject `LoggingHandler` or custom handlers.
pub fn new_task_channel(buffer: usize) -> (ResilientTaskPublisher, TaskWorker) {
    let (tx, rx) = mpsc::channel(buffer);
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
