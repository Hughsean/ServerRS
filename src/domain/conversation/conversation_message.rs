use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ConversationMessage {
    pub id: u64,
    pub conversation_id: u64,
    pub sender_role: String, // user|assistant|system|plugin
    pub sender_user_id: Option<u64>,
    pub message_type: String, // text|image|event
    pub content: String,      // JSON string
    pub token_count: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewConversationMessage {
    pub conversation_id: u64,
    pub sender_role: String,
    pub sender_user_id: Option<u64>,
    pub message_type: String,
    pub content: String,
    pub token_count: Option<i32>,
}
