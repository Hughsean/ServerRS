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
                    info!(username = %t.username, "登录：成功");
                } else {
                    warn!(username = %t.username, reason = %t.reason.as_deref().unwrap_or("?"), "登录：失败");
                }
            }
            TaskEvent::UserRegistered(t) => {
                info!(user_id = t.user_id, username = %t.username, "用户已注册")
            }
            TaskEvent::RefreshTokenRevoked(t) => {
                info!(user_id = t.user_id, username = %t.username, "令牌已撤销")
            }
            TaskEvent::RefreshTokenRotated(t) => {
                info!(user_id = t.user_id, username = %t.username, "令牌已轮换")
            }
            TaskEvent::ConversationCreated(t) => info!(
                conversation_id = t.conversation_id,
                user_id = t.user_id,
                "conversation created"
            ),
            TaskEvent::RiskDetected(t) => {
                if t.risk_level == RiskLevel::Crisis || t.risk_level == RiskLevel::High {
                    warn!(user_id = t.user_id, risk_level = ?t.risk_level, confidence = t.confidence, "高风险");
                } else {
                    info!(user_id = t.user_id, risk_level = ?t.risk_level, confidence = t.confidence, "检测到风险");
                }
            }
            TaskEvent::TurnClosed(_) => {
                tracing::debug!("轮次已关闭");
            }
        }
    }
}
