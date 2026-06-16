use serde::{Deserialize, Serialize};

/// Trigger policy for a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerPolicy {
    /// Only respond when @-mentioned.
    Mention,
    /// Respond when specific keywords are matched.
    Keyword,
    /// Respond to command-style messages (e.g. /ask).
    Command,
    /// Respond to every message (test groups only).
    Always,
    /// Record messages but never respond.
    Silent,
}

/// Reply policy for a group — controls response style and rate.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReplyPolicy {
    /// Cooldown in seconds between responses in the same group.
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_secs: u64,
    /// Max segments per reply.
    #[serde(default = "default_max_segments")]
    pub max_segments: u32,
    /// Max characters per segment.
    #[serde(default = "default_max_chars_per_segment")]
    pub max_chars_per_segment: u32,
    /// Whether to allow the bot to proactively send messages (not just reply).
    #[serde(default)]
    pub allow_proactive: bool,
    /// Keywords that trigger a response (when policy is Keyword).
    #[serde(default)]
    pub keywords: Vec<String>,
}

fn default_cooldown_secs() -> u64 { 30 }
fn default_max_segments() -> u32 { 5 }
fn default_max_chars_per_segment() -> u32 { 80 }

/// Group-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupConfig {
    pub qq_group_id: i64,
    pub group_name: Option<String>,
    pub bot_account_id: u64,
    pub enabled: bool,
    pub trigger_policy: TriggerPolicy,
    pub reply_policy: ReplyPolicy,
    /// How to handle memories for this group.
    pub memory_policy: MemoryPolicy,
}

/// How memories are handled for a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPolicy {
    Off,
    GroupOnly,
    OptInUser,
}

/// External QQ user (not necessarily a registered system user).
#[derive(Debug, Clone)]
pub struct ExternalUser {
    pub qq_user_id: i64,
    pub internal_user_id: Option<u64>,
    pub nickname: Option<String>,
    pub avatar_url: Option<String>,
    pub last_seen_at: Option<i64>,
    pub memory_enabled: bool,
    pub persona_enabled: bool,
}

/// QQ group member (membership within a specific group).
#[derive(Debug, Clone)]
pub struct GroupMember {
    pub qq_group_id: i64,
    pub qq_user_id: i64,
    pub card: Option<String>,
    pub nickname: Option<String>,
    pub role: Option<String>,   // owner | admin | member
    pub title: Option<String>,
    pub join_time: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub status: String,         // active | left | kicked | unknown
}
