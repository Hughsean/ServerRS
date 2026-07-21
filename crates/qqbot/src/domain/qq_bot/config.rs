use serde::{Deserialize, Serialize};

/// 群组的触发策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerPolicy {
    /// 仅在被 @ 时回复。
    Mention,
    /// 匹配到特定关键词时回复。
    Keyword,
    /// 回复命令风格的消息（如 /ask）。
    Command,
    /// 回复每一条消息（仅测试群）。
    Always,
    /// 记录消息但不回复。
    Silent,
}

/// 群组的回复策略 — 控制回复风格和频率。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReplyPolicy {
    /// 同一群组中回复之间的冷却时间（秒）。
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_secs: u64,
    /// 每条回复的最大段数。
    #[serde(default = "default_max_segments")]
    pub max_segments: u32,
    /// 每段的最大字符数。
    #[serde(default = "default_max_chars_per_segment")]
    pub max_chars_per_segment: u32,
    /// 是否允许机器人主动发送消息（不仅仅是回复）。
    #[serde(default)]
    pub allow_proactive: bool,
    /// 触发回复的关键词（当策略为 Keyword 时）。
    #[serde(default)]
    pub keywords: Vec<String>,
}

fn default_cooldown_secs() -> u64 {
    30
}
fn default_max_segments() -> u32 {
    5
}
fn default_max_chars_per_segment() -> u32 {
    80
}

/// 群组级别配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupConfig {
    pub qq_group_id: i64,
    pub group_name: Option<String>,
    pub bot_account_id: u64,
    pub enabled: bool,
    pub trigger_policy: TriggerPolicy,
    pub reply_policy: ReplyPolicy,
    /// 如何处理此群的记忆。
    pub memory_policy: MemoryPolicy,
}

/// 群组记忆的处理方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPolicy {
    Off,
    GroupOnly,
    OptInUser,
}

/// 外部 QQ 用户（不一定是系统注册用户）。
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

/// QQ 群成员（特定群内的成员身份）。
#[derive(Debug, Clone)]
pub struct GroupMember {
    pub qq_group_id: i64,
    pub qq_user_id: i64,
    pub card: Option<String>,
    pub nickname: Option<String>,
    pub role: Option<String>, // owner | admin | member
    pub title: Option<String>,
    pub join_time: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub status: String, // active | left | kicked | unknown
}
