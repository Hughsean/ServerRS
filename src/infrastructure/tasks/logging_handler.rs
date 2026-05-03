use async_trait::async_trait;
use tracing::{info, warn};

use crate::domain::risk::detection_types::RiskLevel;
use crate::domain::tasks::task_event::TaskEvent;
use crate::domain::tasks::task_handler::TaskHandler;

pub struct LoggingHandler;

#[async_trait]
impl TaskHandler for LoggingHandler {
    fn name(&self) -> &str {
        "LoggingHandler"
    }

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
                if t.risk_level == RiskLevel::Crisis || t.risk_level == RiskLevel::High {
                    warn!(user_id = t.user_id, risk_level = ?t.risk_level, confidence = t.confidence, "HIGH RISK");
                } else {
                    info!(user_id = t.user_id, risk_level = ?t.risk_level, confidence = t.confidence, "risk detected");
                }
            }
        }
    }
}
