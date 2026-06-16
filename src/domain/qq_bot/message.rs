use serde::{Deserialize, Serialize};

/// Direction of a group message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    Inbound,
    Outbound,
}

/// Processing status of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Pending,
    Ignored,
    Processed,
    Failed,
}

/// A single segment inside a QQ message (e.g. text, image, face).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageSegment {
    Text { content: String },
    Face { id: i32, text: Option<String> },
    Image { file: String, url: Option<String> },
    At { qq: String },
    Reply { id: String },
    Record { file: String },
    Video { file: String },
    File { file: String, name: Option<String> },
    Unknown { raw: String },
}

/// Normalized group message, ready for LLM consumption.
#[derive(Debug, Clone)]
pub struct NormalizedMessage {
    /// Internal DB id (populated after persistence).
    pub id: Option<u64>,
    /// Which bot account received this.
    pub bot_account_id: u64,
    /// QQ group id.
    pub qq_group_id: i64,
    /// Sender QQ (may be None for outbound).
    pub qq_user_id: Option<i64>,
    /// Platform message id (used for idempotency).
    pub platform_message_id: String,
    /// inbound | outbound.
    pub direction: MessageDirection,
    /// Raw original text (from CQ code or plain text).
    pub raw_text: String,
    /// Cleaned text, with CQ codes removed and @bot stripped.
    pub normalized_text: String,
    /// Parsed message segments.
    pub segments: Vec<MessageSegment>,
    /// Whether the bot was @-mentioned.
    pub at_bot: bool,
    /// Recognized command (e.g. "bind", "help"), if any.
    pub command_name: Option<String>,
    /// Unix timestamp of the message.
    pub sent_at: i64,
}
