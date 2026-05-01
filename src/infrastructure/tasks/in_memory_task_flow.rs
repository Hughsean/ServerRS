use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::domain::tasks::task_event::TaskEvent;
use crate::domain::tasks::task_handler::TaskHandler;
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::shared::error::AppError;

// ── Resilient publisher ──

#[derive(Clone)]
pub struct ResilientTaskPublisher {
    sender: mpsc::Sender<TaskEvent>,
    overflow_limit: usize,
}

impl ResilientTaskPublisher {
    fn new(sender: mpsc::Sender<TaskEvent>, overflow_limit: usize) -> Self {
        Self {
            sender,
            overflow_limit,
        }
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

// ── Logging handler ──

pub struct LoggingHandler;

#[async_trait]
impl TaskHandler for LoggingHandler {
    async fn handle(&self, event: &TaskEvent) {
        match event {
            TaskEvent::LoginAudit(t) => {
                if t.success {
                    info!(username = %t.username, "login: success");
                } else {
                    warn!(username = %t.username, reason = %t.reason.as_deref().unwrap_or("?"), "login: failed");
                }
            }
            TaskEvent::UserRegistered(t) => {
                info!(user_id = t.user_id, username = %t.username, "user registered")
            }
            TaskEvent::RefreshTokenRevoked(t) => {
                info!(user_id = t.user_id, username = %t.username, "token revoked")
            }
            TaskEvent::RefreshTokenRotated(t) => {
                info!(user_id = t.user_id, username = %t.username, "token rotated")
            }
            TaskEvent::SessionCreated(t) => {
                info!(session_id = %t.session_id, user_id = t.user_id, "session created")
            }
            TaskEvent::SessionExpired(t) => {
                info!(session_id = %t.session_id, user_id = t.user_id, "session expired")
            }
            TaskEvent::ConversationCreated(t) => info!(
                conversation_id = t.conversation_id,
                user_id = t.user_id,
                "conversation created"
            ),
            TaskEvent::RiskDetected(t) => {
                if t.risk_level == "Crisis" || t.risk_level == "High" {
                    warn!(user_id = t.user_id, risk_level = %t.risk_level, confidence = t.confidence, "HIGH RISK");
                } else {
                    info!(user_id = t.user_id, risk_level = %t.risk_level, confidence = t.confidence, "risk detected");
                }
            }
        }
    }
}

// ── Worker ──

pub struct TaskWorker {
    receiver: mpsc::Receiver<TaskEvent>,
    handlers: Vec<Arc<dyn TaskHandler>>,
}

pub fn new_task_channel(buffer: usize) -> (ResilientTaskPublisher, TaskWorker) {
    let (tx, rx) = mpsc::channel(buffer);
    let worker = TaskWorker {
        receiver: rx,
        handlers: vec![Arc::new(LoggingHandler)],
    };
    (ResilientTaskPublisher::new(tx, buffer), worker)
}

impl TaskWorker {
    #[allow(dead_code)]
    pub fn with_handler(mut self, handler: Arc<dyn TaskHandler>) -> Self {
        self.handlers.push(handler);
        self
    }

    pub async fn run(mut self) {
        while let Some(event) = self.receiver.recv().await {
            for h in &self.handlers {
                h.handle(&event).await;
            }
        }
        info!("task worker stopped");
    }
}
