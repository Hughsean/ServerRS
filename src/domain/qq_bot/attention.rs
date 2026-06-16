use serde::{Deserialize, Serialize};

/// Attention state of the QQ bot — it can only engage one group at a time,
/// mimicking human attention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttentionState {
    /// Bot is idle, ready to engage any group.
    Idle,
    /// Bot is engaging with a specific group, but not yet committed.
    Engaging(i64),
    /// Bot is actively in conversation with a group.
    Engaged(i64),
    /// Bot is cooling down after a conversation (group_id, cooldown_until_epoch_ms).
    Cooldown(i64, u64),
}

/// Decision made by the trigger evaluator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerDecision {
    /// Skip this message entirely.
    Skip,
    /// Wait — message is noted but no action taken right now.
    Wait,
    /// Respond to this message.
    Respond,
}

/// Configuration for a single bot account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotAccount {
    pub bot_account_id: u64,
    /// Platform (e.g. "qq").
    pub platform: String,
    /// QQ number of the bot.
    pub self_qq_id: i64,
    pub display_name: Option<String>,
    pub adapter: String,
    /// websocket | http | webhook.
    pub connection_mode: String,
    pub enabled: bool,
}
