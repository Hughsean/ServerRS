use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 代理轮次的触发类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerType {
    Mention,
    Keyword,
    Command,
    Always,
    Manual,
}

/// 代理轮次的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Created,
    Responded,
    Failed,
    Cancelled,
}

/// 由群消息触发的单次代理调用记录。
#[derive(Debug, Clone)]
pub struct AgentTurn {
    pub turn_id: Option<u64>,
    pub bot_account_id: u64,
    pub qq_group_id: i64,
    /// 触发代理的入站消息 ID。
    pub trigger_message_id: u64,
    /// 作为回复发送的出站消息 ID（如果有）。
    pub response_message_id: Option<u64>,
    pub trigger_type: TriggerType,
    pub qq_user_id: Option<i64>,
    pub internal_user_id: Option<u64>,
    pub prompt_version: Option<String>,
    pub model_name: Option<String>,
    pub reasoning_enabled: Option<bool>,
    pub input_token_count: Option<u32>,
    pub output_token_count: Option<u32>,
    pub latency_ms: Option<u32>,
    pub status: TurnStatus,
    pub error_message: Option<String>,
    pub trace_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
