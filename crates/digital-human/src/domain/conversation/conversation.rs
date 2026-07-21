use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Conversation {
    pub id: u64,
    pub user_id: u64,
    pub title: Option<String>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub message_count: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewConversation {
    pub user_id: u64,
    pub title: Option<String>,
}
