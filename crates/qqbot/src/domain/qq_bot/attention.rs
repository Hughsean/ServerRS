use serde::{Deserialize, Serialize};

/// QQ 机器人的注意力状态 — 一次只能与一个群互动，
/// 模拟人类注意力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttentionState {
    /// 机器人空闲，准备与任何群互动。
    Idle,
    /// 机器人正在与特定群互动，但尚未确定。
    Engaging(i64),
    /// 机器人正在与一个群积极对话。
    Engaged(i64),
    /// 机器人对话后正在冷却（group_id, cooldown_until_epoch_ms）。
    Cooldown(i64, u64),
}

/// 触发器评估器做出的决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerDecision {
    /// 完全跳过此消息。
    Skip,
    /// 等待 — 消息已记录但暂不处理。
    Wait,
    /// 回复此消息。
    Respond,
}

/// 单个机器人账号的配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotAccount {
    pub bot_account_id: u64,
    /// 平台（如 "qq"）。
    pub platform: String,
    /// 机器人的 QQ 号。
    pub self_qq_id: i64,
    pub display_name: Option<String>,
    pub adapter: String,
    /// websocket | http | webhook。
    pub connection_mode: String,
    pub enabled: bool,
}
