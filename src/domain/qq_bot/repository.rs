use async_trait::async_trait;

use super::attention::BotAccount;
use super::config::{ExternalUser, GroupConfig, GroupMember, TriggerPolicy};
use super::message::NormalizedMessage;
use super::turn::AgentTurn;

// ── BotAccount Repository ───────────────────────────────────────────────

#[async_trait]
pub trait BotAccountRepository: Send + Sync {
    async fn find_by_self_qq_id(
        &self,
        self_qq_id: i64,
    ) -> Result<Option<BotAccount>, crate::shared::error::AppError>;
    async fn find_enabled(&self) -> Result<Vec<BotAccount>, crate::shared::error::AppError>;
    async fn upsert(
        &self,
        account: &BotAccount,
    ) -> Result<BotAccount, crate::shared::error::AppError>;
}

// ── ExternalUser Repository ─────────────────────────────────────────────

#[async_trait]
pub trait ExternalUserRepository: Send + Sync {
    async fn find_by_qq_user_id(
        &self,
        qq_user_id: i64,
    ) -> Result<Option<ExternalUser>, crate::shared::error::AppError>;
    async fn upsert(
        &self,
        user: &ExternalUser,
    ) -> Result<ExternalUser, crate::shared::error::AppError>;
    async fn update_last_seen(
        &self,
        qq_user_id: i64,
        last_seen_at: i64,
    ) -> Result<(), crate::shared::error::AppError>;
}

// ── Group Repository ────────────────────────────────────────────────────

#[async_trait]
pub trait GroupRepository: Send + Sync {
    async fn find_by_group_id(
        &self,
        qq_group_id: i64,
    ) -> Result<Option<GroupConfig>, crate::shared::error::AppError>;
    async fn find_enabled_by_bot(
        &self,
        bot_account_id: u64,
    ) -> Result<Vec<GroupConfig>, crate::shared::error::AppError>;
    async fn upsert(
        &self,
        group: &GroupConfig,
    ) -> Result<GroupConfig, crate::shared::error::AppError>;
    async fn update_last_seen(
        &self,
        qq_group_id: i64,
        last_seen_at: i64,
    ) -> Result<(), crate::shared::error::AppError>;
    async fn update_trigger_policy(
        &self,
        qq_group_id: i64,
        policy: TriggerPolicy,
    ) -> Result<(), crate::shared::error::AppError>;
    async fn set_enabled(
        &self,
        qq_group_id: i64,
        enabled: bool,
    ) -> Result<(), crate::shared::error::AppError>;
}

// ── GroupMember Repository ──────────────────────────────────────────────

#[async_trait]
pub trait GroupMemberRepository: Send + Sync {
    async fn find(
        &self,
        qq_group_id: i64,
        qq_user_id: i64,
    ) -> Result<Option<GroupMember>, crate::shared::error::AppError>;
    async fn upsert(
        &self,
        member: &GroupMember,
    ) -> Result<GroupMember, crate::shared::error::AppError>;
    async fn update_last_seen(
        &self,
        qq_group_id: i64,
        qq_user_id: i64,
        last_seen_at: i64,
    ) -> Result<(), crate::shared::error::AppError>;
    async fn list_by_group(
        &self,
        qq_group_id: i64,
    ) -> Result<Vec<GroupMember>, crate::shared::error::AppError>;
}

// ── GroupMessage Repository ─────────────────────────────────────────────

#[async_trait]
pub trait GroupMessageRepository: Send + Sync {
    /// 插入标准化后的消息。返回内部 ID。
    /// 幂等性：如果 `platform_message_id` + `bot_account_id` 已存在，返回现有记录。
    async fn insert(
        &self,
        msg: &NormalizedMessage,
    ) -> Result<NormalizedMessage, crate::shared::error::AppError>;
    async fn find_by_platform_id(
        &self,
        bot_account_id: u64,
        platform_message_id: &str,
    ) -> Result<Option<NormalizedMessage>, crate::shared::error::AppError>;
    /// 获取群组的最近消息（用于上下文构建）。
    async fn recent_by_group(
        &self,
        qq_group_id: i64,
        limit: u32,
    ) -> Result<Vec<NormalizedMessage>, crate::shared::error::AppError>;
    /// 更新处理状态。
    async fn update_status(
        &self,
        id: u64,
        status: super::message::ProcessStatus,
        error: Option<&str>,
    ) -> Result<(), crate::shared::error::AppError>;
}

// ── AgentTurn Repository ────────────────────────────────────────────────

#[async_trait]
pub trait AgentTurnRepository: Send + Sync {
    async fn insert(&self, turn: &AgentTurn) -> Result<AgentTurn, crate::shared::error::AppError>;
    async fn update_response(
        &self,
        turn_id: u64,
        response_message_id: u64,
        status: super::turn::TurnStatus,
    ) -> Result<(), crate::shared::error::AppError>;
    async fn update_status(
        &self,
        turn_id: u64,
        status: super::turn::TurnStatus,
        error: Option<&str>,
    ) -> Result<(), crate::shared::error::AppError>;
    async fn find_by_trace_id(
        &self,
        trace_id: &str,
    ) -> Result<Option<AgentTurn>, crate::shared::error::AppError>;
    async fn recent_by_group(
        &self,
        qq_group_id: i64,
        limit: u32,
    ) -> Result<Vec<AgentTurn>, crate::shared::error::AppError>;
}

// ── Outbox Repository ───────────────────────────────────────────────────

#[async_trait]
pub trait OutboxRepository: Send + Sync {
    async fn insert(
        &self,
        entry: &OutboxEntry,
    ) -> Result<OutboxEntry, crate::shared::error::AppError>;
    /// 获取下一批等待发送的条目。
    async fn fetch_due(
        &self,
        limit: u32,
    ) -> Result<Vec<OutboxEntry>, crate::shared::error::AppError>;
    async fn mark_sent(
        &self,
        outbox_id: u64,
        platform_message_id: &str,
    ) -> Result<(), crate::shared::error::AppError>;
    async fn mark_failed(
        &self,
        outbox_id: u64,
        error: &str,
    ) -> Result<(), crate::shared::error::AppError>;
    async fn mark_cancelled(&self, outbox_id: u64) -> Result<(), crate::shared::error::AppError>;
}

/// 用于可靠消息发送的发件箱条目。
#[derive(Debug, Clone)]
pub struct OutboxEntry {
    pub outbox_id: Option<u64>,
    pub bot_account_id: u64,
    pub qq_group_id: Option<i64>,
    pub qq_user_id: Option<i64>,
    /// "group" | "private"
    pub target_type: String,
    /// 要发送的 JSON 负载
    pub payload: serde_json::Value,
    pub related_turn_id: Option<u64>,
    pub status: OutboxStatus,
    pub attempts: u32,
    pub max_attempts: u32,
    pub next_run_at: i64,
    pub platform_message_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxStatus {
    Pending,
    Sending,
    Sent,
    Failed,
    Cancelled,
}

// ── GroupSummary Repository ─────────────────────────────────────────────

#[async_trait]
pub trait GroupSummaryRepository: Send + Sync {
    /// 查找群组的活跃滚动摘要。
    async fn find_active_rolling(
        &self,
        qq_group_id: i64,
    ) -> Result<Option<GroupSummary>, crate::shared::error::AppError>;
    async fn insert(
        &self,
        summary: &GroupSummary,
    ) -> Result<GroupSummary, crate::shared::error::AppError>;
    async fn disable(&self, summary_id: u64) -> Result<(), crate::shared::error::AppError>;
}

/// 群组摘要记录。
#[derive(Debug, Clone)]
pub struct GroupSummary {
    pub summary_id: Option<u64>,
    pub qq_group_id: i64,
    pub summary_type: String, // "rolling_group" | "milestone_group"
    pub content: String,
    pub message_start_id: u64,
    pub message_end_id: u64,
    pub supersedes_id: Option<u64>,
    pub token_count: Option<u32>,
    pub status: bool, // true = active
    pub vector_id: Option<String>,
}

// ── GroupMemory Repository ──────────────────────────────────────────────

#[async_trait]
pub trait GroupMemoryRepository: Send + Sync {
    /// 查找群组的活跃记忆，按显著性排序。
    async fn find_active_by_group(
        &self,
        qq_group_id: i64,
        limit: u32,
    ) -> Result<Vec<GroupMemory>, crate::shared::error::AppError>;
    async fn upsert(
        &self,
        memory: &GroupMemory,
    ) -> Result<GroupMemory, crate::shared::error::AppError>;
    async fn disable(&self, group_memory_id: u64) -> Result<(), crate::shared::error::AppError>;
}

/// 群组级别记忆。
#[derive(Debug, Clone)]
pub struct GroupMemory {
    pub group_memory_id: Option<u64>,
    pub qq_group_id: i64,
    pub memory_key: Option<String>,
    pub canonical_form: Option<String>,
    pub memory_type: String, // group_preference | group_fact | group_rule | recurring_topic | inside_joke
    pub content: String,
    pub confidence: f64,
    pub salience: f64,
    pub source_message_id: Option<u64>,
    pub reinforce_count: u32,
    pub status: i8, // 1=active, 0=disabled, -1=contradicted
}
