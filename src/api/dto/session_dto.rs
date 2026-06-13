use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct SessionCreateRequest {
    #[serde(default)]
    #[validate(range(min = 0))]
    pub user_id: u64,
    pub dialogue_id: Option<u64>,
    pub location: Option<std::collections::HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Serialize)]
pub struct SessionCreateResponse {
    pub session_id: String,
    pub prompt: String,
    pub location: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub user_profile: Option<serde_json::Value>,
    pub timeout_seconds: u64,
    pub dialogue_id: Option<u64>,
}

#[derive(Debug, Deserialize, Validate)]
pub struct MessageRequest {
    #[validate(length(min = 1))]
    pub text: String,
    pub emotion: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub session_id: String,
    pub reply: String,
    pub session_closed: bool,
    pub dialogue_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionStatusResponse {
    pub session_id: String,
    pub user_id: u64,
    pub dialogue_id: Option<u64>,
    pub timeout_seconds: u64,
}

// ── Conversation list ──

#[derive(Debug, Serialize)]
pub struct ConversationResponse {
    pub id: u64,
    pub user_id: u64,
    pub title: Option<String>,
    pub is_title_generated: bool,
    pub last_message_at: Option<String>,
    pub message_count: i32,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct ConversationMessageResponse {
    pub id: u64,
    pub conversation_id: u64,
    pub sender_role: String,
    pub sender_user_id: Option<u64>,
    pub message_type: String,
    pub content: String,
    pub token_count: Option<i32>,
    pub created_at: String,
}
