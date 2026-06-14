use serde::{Deserialize, Serialize};
use validator::Validate;

// ── POST /api/v1/chat/open ──

#[derive(Debug, Deserialize, Validate)]
pub struct ChatOpenRequest {
    // No required fields; user_id comes from Bearer token.
}

#[derive(Debug, Serialize)]
pub struct ChatOpenResponse {
    pub conversation_id: u64,
    pub message_count: u64,
    pub title: Option<String>,
    pub created_at: String,
}

// ── POST /api/v1/chat/messages ──

#[derive(Debug, Deserialize, Validate)]
pub struct ChatMessageRequest {
    #[validate(length(min = 1))]
    pub text: String,
    pub emotion: Option<String>,
    #[serde(default)]
    pub location: Option<std::collections::HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Serialize)]
pub struct ChatMessageResponse {
    pub conversation_id: u64,
    pub reply: String,
}

// ── GET /api/v1/chat/history ──

#[derive(Debug, Serialize)]
pub struct ChatHistoryResponse {
    pub conversation_id: u64,
    pub messages: Vec<ChatMessageItem>,
}

#[derive(Debug, Serialize)]
pub struct ChatMessageItem {
    pub id: u64,
    pub sender_role: String,
    pub content: String,
    pub created_at: String,
}

// ── GET /api/v1/chat/memories ──

#[derive(Debug, Serialize)]
pub struct ChatMemoryResponse {
    pub memories: Vec<ChatMemoryItem>,
}

#[derive(Debug, Serialize)]
pub struct ChatMemoryItem {
    pub memory_id: u64,
    pub memory_type: String,
    pub content: String,
    pub confidence: f64,
    pub created_at: String,
}

// ── GET /api/v1/chat/persona ──

#[derive(Debug, Serialize)]
pub struct ChatPersonaResponse {
    pub snapshot_id: Option<u64>,
    pub snapshot_data: Option<serde_json::Value>,
}

// ── POST /api/v1/chat/memory/{id}/disable ──

#[derive(Debug, Serialize)]
pub struct DisableMemoryResponse {
    pub memory_id: u64,
    pub disabled: bool,
}

// ── POST /api/v1/chat/persona/reset ──

#[derive(Debug, Serialize)]
pub struct PersonaResetResponse {
    pub reset: bool,
}

// ── POST /api/v1/chat/persona/rebuild ──

#[derive(Debug, Serialize)]
pub struct PersonaRebuildResponse {
    pub snapshot_id: Option<u64>,
}

// ── POST /api/v1/chat/transcript/clear ──

#[derive(Debug, Serialize)]
pub struct TranscriptClearResponse {
    pub cleared: bool,
}

// ── POST /api/v1/chat/forget ──

#[derive(Debug, Serialize)]
pub struct ForgetResponse {
    pub forgotten: bool,
}
