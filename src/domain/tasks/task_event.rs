use crate::domain::risk::detection_types::RiskLevel;
use chrono::Utc;

#[derive(Debug, Clone)]
pub enum TaskEvent {
    // ── Auth audit ──
    LoginAudit(LoginAuditTask),
    UserRegistered(UserRegisteredTask),
    RefreshTokenRevoked(RefreshTokenRevokedTask),
    RefreshTokenRotated(RefreshTokenRotatedTask),

    // ── Conversation ──
    ConversationCreated(ConversationLifecycleTask),

    // ── Risk detection ──
    RiskDetected(RiskDetectedTask),

    // ── Conversation turn lifecycle ──
    TurnClosed(TurnClosedEvent),
}

// ── Auth payloads ──

#[derive(Debug, Clone)]
pub struct LoginAuditTask {
    pub username: String,
    pub success: bool,
    pub reason: Option<String>,
    pub device_id: Option<String>,
}

impl LoginAuditTask {
    pub fn succeeded(username: String, device_id: Option<String>) -> Self {
        Self {
            username,
            success: true,
            reason: None,
            device_id,
        }
    }

    pub fn failed(username: String, device_id: Option<String>, reason: impl Into<String>) -> Self {
        Self {
            username,
            success: false,
            reason: Some(reason.into()),
            device_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserRegisteredTask {
    pub user_id: u64,
    pub username: String,
    pub device_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RefreshTokenRevokedTask {
    pub user_id: u64,
    pub username: String,
    pub token_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RefreshTokenRotatedTask {
    pub user_id: u64,
    pub username: String,
    pub old_token_id: String,
    pub device_id: Option<String>,
}

// ── Conversation payloads ──

#[derive(Debug, Clone)]
pub struct ConversationLifecycleTask {
    pub conversation_id: u64,
    pub user_id: u64,
}

// ── Risk detection payloads ──

#[derive(Debug, Clone)]
pub struct RiskDetectedTask {
    pub user_id: u64,
    pub conversation_id: Option<u64>,
    pub risk_level: RiskLevel,
    pub confidence: f64,
}

// ── Conversation turn lifecycle payloads ──

/// 在聊天轮次完成后发布: user message + assistant message both persisted,
/// HTTP/SSE connection closed, no further effect on current reply.
#[derive(Debug, Clone)]
pub struct TurnClosedEvent {
    pub user_id: u64,
    pub conversation_id: u64,
    pub user_message_id: Option<u64>,
    pub assistant_message_id: Option<u64>,
    pub closed_at: chrono::DateTime<Utc>,
}
