use serde::{Deserialize, Serialize};
use validator::Validate;

// ── POST /api/v1/chat/open ──

#[derive(Debug, Deserialize, Validate)]
pub struct ChatOpenRequest {
    // No required fields; user_id comes from Bearer token.
}

#[derive(Debug, Serialize)]
pub struct ChatOpenResponse {
    pub conversation: ChatConversationInfo,
    pub personalization_enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct ChatConversationInfo {
    pub id: u64,
    pub message_count: u64,
    pub last_message_at: Option<String>,
}

// ── POST /api/v1/chat/messages ──

#[derive(Debug, Deserialize, Validate)]
pub struct ChatMessageRequest {
    #[validate(length(min = 1))]
    pub text: String,
    #[validate(length(max = 200))]
    pub emotion: Option<String>,
    #[serde(default)]
    pub location: Option<std::collections::HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Serialize)]
pub struct ChatMessageResponse {
    pub conversation_id: u64,
    pub reply: String,
    pub tool_calls: Vec<ChatToolCallItem>,
}

#[derive(Debug, Serialize)]
pub struct ChatToolCallItem {
    pub name: String,
    pub arguments: serde_json::Value,
}

// ── GET /api/v1/chat/history ──

#[derive(Debug, Deserialize)]
pub struct ChatHistoryQuery {
    pub before_id: Option<u64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ChatHistoryResponse {
    pub messages: Vec<ChatMessageItem>,
    pub next_before_id: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ChatMessageItem {
    pub id: u64,
    pub sender_role: String,
    pub message_type: String,
    pub content: serde_json::Value,
    pub created_at: String,
}

// ── GET /api/v1/chat/memories ──

#[derive(Debug, Deserialize)]
pub struct ChatMemoryQuery {
    #[serde(rename = "type")]
    pub memory_types: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ChatMemoryResponse {
    pub memories: Vec<ChatMemoryItem>,
    pub total_active: usize,
}

#[derive(Debug, Serialize)]
pub struct ChatMemoryItem {
    pub memory_id: u64,
    pub memory_type: String,
    pub content: String,
    pub confidence: f64,
    pub reinforce_count: u32,
    pub created_at: String,
    pub reinforced_at: Option<String>,
}

// ── GET /api/v1/chat/persona ──

#[derive(Debug, Serialize)]
pub struct ChatPersonaResponse {
    pub has_active_persona: bool,
    pub generated_at: Option<String>,
    pub snapshot_summary: ChatPersonaSnapshotSummary,
    pub personalization_enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct ChatPersonaSnapshotSummary {
    pub communication_preferences_count: usize,
    pub stable_facts_count: usize,
    pub recurring_topics_count: usize,
    pub goals_count: usize,
    pub sensitive_context_count: usize,
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
    pub snapshot_id: u64,
}

// ── POST /api/v1/chat/transcript/clear ──

#[derive(Debug, Serialize)]
pub struct TranscriptClearResponse {
    pub cleared_messages: bool,
    pub cleared_summaries: bool,
    pub memories_preserved: bool,
    pub persona_preserved: bool,
    pub post_risk_audits_cleared: bool,
}

// ── POST /api/v1/chat/forget ──

#[derive(Debug, Serialize)]
pub struct ForgetResponse {
    pub messages_cleared: bool,
    pub summaries_cleared: bool,
    pub memories_disabled: u64,
    pub persona_expired: bool,
    pub post_risk_audits_deleted: bool,
    pub personalization_disabled: bool,
}
