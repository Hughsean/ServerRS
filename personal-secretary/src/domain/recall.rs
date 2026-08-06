//! 协议无关的消息撤回领域模型。
//!
//! 本模块描述撤回事件、tombstone 记录和关联键，不依赖 NapCat、OneBot、QQ、SeaORM、
//! MySQL、Axum、Tokio 或任何 HTTP 客户端。
//!
//! 核心不变量：
//! - 撤回通知成为可审计 SourceEvent，不能只记录 Debug 后丢弃。
//! - 使用 `(account_id, channel, conversation, platform_message_id)` 组合关联原消息，
//!   **禁止单 message_id 跨账号关联**（任务七-5）。
//! - 撤回先到时保存 pending tombstone；原消息后到后自动关联、失效且保持幂等。
//! - 不物理删除审计历史：保留撤回事件、原消息信封、被撤回状态、失效原因、来源关系、投影时间。
//! - 已确认事实不能静默删除，标记来源撤回并进入复核/冲突回读状态，保留修订链。

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ConversationRef, MessageSource, SourceAccountRef};

/// 撤回事件的唯一标识。撤回本身也是一条 SourceEvent，此 ID 与 SourceEventId 一致。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecallEventId(String);

impl RecallEventId {
    /// 统一事件身份必须能放入 `CHAR(36)`（与 `SourceEventId` 同宽）。
    /// 生产路径应使用 UUID；禁止把账号/群/消息 ID 直接拼进主键。
    pub fn new(value: impl Into<String>) -> Result<Self, RecallError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(RecallError::InvalidIdentity(
                "recall_event_id must not be empty".into(),
            ));
        }
        if trimmed.len() > 36 {
            return Err(RecallError::InvalidIdentity(format!(
                "recall_event_id exceeds CHAR(36) limit (len={})",
                trimmed.len()
            )));
        }
        // 拒绝首尾空白；内部保留调用方原串（已 trim 后的稳定形式）。
        if trimmed.len() != value.len() {
            return Err(RecallError::InvalidIdentity(
                "recall_event_id must not contain leading/trailing whitespace".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 撤回类型：群撤回或好友撤回。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallKind {
    /// 群消息撤回（`group_recall`）。
    Group,
    /// 好友消息撤回（`friend_recall`）。
    Friend,
}

impl RecallKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Friend => "friend",
        }
    }

    pub fn parse_from_str(value: &str) -> Option<Self> {
        match value {
            "group" => Some(Self::Group),
            "friend" => Some(Self::Friend),
            _ => None,
        }
    }
}

/// 撤回事件的关联键：用于匹配被撤回的原消息。
///
/// **禁止单 message_id 跨账号关联**（任务七-5）。同一平台 message_id 在不同账号
/// 下是不同的消息，必须用 `(account, channel, conversation, platform_message_id)`
/// 四元组关联。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallCorrelationKey {
    pub account: SourceAccountRef,
    pub channel: MessageSource,
    pub conversation: ConversationRef,
    pub platform_message_id: String,
}

impl RecallCorrelationKey {
    pub fn new(
        account: SourceAccountRef,
        channel: MessageSource,
        conversation: ConversationRef,
        platform_message_id: impl Into<String>,
    ) -> Result<Self, RecallError> {
        let platform_message_id = platform_message_id.into();
        if platform_message_id.trim().is_empty() {
            return Err(RecallError::InvalidIdentity(
                "platform_message_id must not be empty".into(),
            ));
        }
        Ok(Self {
            account,
            channel,
            conversation,
            platform_message_id,
        })
    }

    /// 持久化用的稳定键（不含账号敏感信息，但唯一标识关联）。
    pub fn key_string(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.account.channel.as_str(),
            self.account.account_id,
            self.conversation.kind.as_str(),
            self.conversation.id,
            self.platform_message_id
        )
    }
}

/// 撤回事件领域模型。撤回本身也是一条可审计 SourceEvent。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallEvent {
    /// 撤回事件自身的 ID（与 SourceEventId 一致）。
    pub recall_event_id: RecallEventId,
    /// 执行撤回的账号。
    pub account: SourceAccountRef,
    /// 撤回类型。
    pub kind: RecallKind,
    /// 被撤回原消息的关联键。
    pub correlation: RecallCorrelationKey,
    /// 执行撤回的操作者平台 ID（群撤回时可能是群主/管理员；好友撤回时是发送者本人）。
    pub operator_platform_id: Option<String>,
    /// 撤回发生时间（Unix 秒）。
    pub occurred_at_unix_secs: i64,
}

/// 从持久化 inbox 领取的一条撤回事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedRecallEvent {
    pub event: RecallEvent,
    pub lease_token: String,
    pub attempt: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallFailureKind {
    Retryable,
    Permanent,
}

/// Tombstone（墓碑）状态：被撤回原消息的失效记录。
///
/// 不物理删除审计历史。保留撤回事件、原消息信封、被撤回状态、失效原因、来源关系、投影时间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TombstoneStatus {
    /// 撤回先到，原消息尚未到达。等待原消息入库后自动关联。
    Pending,
    /// 已应用失效：原消息已标记为被撤回，正文不再返回。
    Applied,
    /// 幂等重放：相同撤回再次到达时返回已应用状态，不重复处理。
    IdempotentReapply,
}

impl TombstoneStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::IdempotentReapply => "idempotent_reapply",
        }
    }

    pub fn parse_from_str(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "applied" => Some(Self::Applied),
            "idempotent_reapply" => Some(Self::IdempotentReapply),
            _ => None,
        }
    }
}

/// Tombstone 记录：被撤回原消息的失效状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TombstoneRecord {
    /// 被撤回原消息的 SourceEvent ID。撤回先到时为 None（pending）。
    pub source_event_id: Option<String>,
    /// 触发此 tombstone 的撤回事件 ID。
    pub recall_event_id: RecallEventId,
    /// 关联键（用于匹配原消息）。
    pub correlation: RecallCorrelationKey,
    /// tombstone 状态。
    pub status: TombstoneStatus,
    /// 失效原因（审计用）。
    pub invalidation_reason: String,
    /// 失效时间（Unix 秒）。
    pub invalidated_at_unix_secs: i64,
    /// tombstone 创建时间（Unix 秒）。
    pub created_at_unix_secs: i64,
}

/// 撤回失效目标：需要失效的派生状态类别（任务七-8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationTarget {
    /// Thread 语义候选（claims/decisions/questions）。
    ThreadSemanticCandidates,
    /// 未确认结论（owner response draft）。
    OwnerResponseDraft,
    /// Planner / Retriever 临时结果。
    PlannerTemporaryResults,
    /// MemoryFact 候选。
    MemoryFactCandidates,
    /// Commitment / FollowUp 候选。
    CommitmentFollowUpCandidates,
    /// Artifact 引用。
    ArtifactReferences,
}

impl InvalidationTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ThreadSemanticCandidates => "thread_semantic_candidates",
            Self::OwnerResponseDraft => "owner_response_draft",
            Self::PlannerTemporaryResults => "planner_temporary_results",
            Self::MemoryFactCandidates => "memory_fact_candidates",
            Self::CommitmentFollowUpCandidates => "commitment_followup_candidates",
            Self::ArtifactReferences => "artifact_references",
        }
    }

    /// 撤回需要失效的全部目标。
    pub fn all() -> &'static [InvalidationTarget] {
        &[
            Self::ThreadSemanticCandidates,
            Self::OwnerResponseDraft,
            Self::PlannerTemporaryResults,
            Self::MemoryFactCandidates,
            Self::CommitmentFollowUpCandidates,
            Self::ArtifactReferences,
        ]
    }
}

/// 撤回领域错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RecallError {
    #[error("invalid recall identity: {0}")]
    InvalidIdentity(String),
    #[error("recall correlation key collision: {0}")]
    CorrelationCollision(String),
    #[error("recall store error: {0}")]
    Store(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConversationKind, ConversationRef};

    fn account(id: &str) -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, id.to_string()).unwrap()
    }

    fn conv(kind: ConversationKind, id: &str) -> ConversationRef {
        ConversationRef::new(kind, id.to_string()).unwrap()
    }

    #[test]
    fn recall_event_id_rejects_empty_and_overlong() {
        assert!(RecallEventId::new("").is_err());
        assert!(RecallEventId::new("  ").is_err());
        assert!(RecallEventId::new("recall-1").is_ok());
        // 真实账号/群/消息拼接会超过 CHAR(36)，必须拒绝。
        let realistic = "recall-group-1839717811-671260344-1234567890123456789";
        assert!(RecallEventId::new(realistic).is_err());
        assert_eq!(realistic.len(), 53);
        assert!(RecallEventId::new("a".repeat(36)).is_ok());
        assert!(RecallEventId::new("a".repeat(37)).is_err());
    }

    #[test]
    fn correlation_key_includes_account_not_just_message_id() {
        // 同一 message_id 在不同账号下是不同的消息。
        let key_a = RecallCorrelationKey::new(
            account("acc-1"),
            MessageSource::NapCat,
            conv(ConversationKind::Group, "g-1"),
            "msg-100",
        )
        .unwrap();
        let key_b = RecallCorrelationKey::new(
            account("acc-2"),
            MessageSource::NapCat,
            conv(ConversationKind::Group, "g-1"),
            "msg-100", // 同一 message_id
        )
        .unwrap();
        // 禁止单 message_id 跨账号关联：两个键必须不相等。
        assert_ne!(key_a, key_b);
        // key_string 也不同。
        assert_ne!(key_a.key_string(), key_b.key_string());
    }

    #[test]
    fn correlation_key_rejects_empty_message_id() {
        assert!(
            RecallCorrelationKey::new(
                account("acc-1"),
                MessageSource::NapCat,
                conv(ConversationKind::Group, "g-1"),
                ""
            )
            .is_err()
        );
        assert!(
            RecallCorrelationKey::new(
                account("acc-1"),
                MessageSource::NapCat,
                conv(ConversationKind::Group, "g-1"),
                "msg-1"
            )
            .is_ok()
        );
    }

    #[test]
    fn same_account_same_message_id_same_conversation_are_equal() {
        let key1 = RecallCorrelationKey::new(
            account("acc-1"),
            MessageSource::NapCat,
            conv(ConversationKind::Group, "g-1"),
            "msg-100",
        )
        .unwrap();
        let key2 = RecallCorrelationKey::new(
            account("acc-1"),
            MessageSource::NapCat,
            conv(ConversationKind::Group, "g-1"),
            "msg-100",
        )
        .unwrap();
        assert_eq!(key1, key2);
    }

    #[test]
    fn tombstone_status_roundtrips() {
        for status in [
            TombstoneStatus::Pending,
            TombstoneStatus::Applied,
            TombstoneStatus::IdempotentReapply,
        ] {
            let s = status.as_str();
            assert_eq!(TombstoneStatus::parse_from_str(s), Some(status));
        }
        assert!(TombstoneStatus::parse_from_str("invalid").is_none());
    }

    #[test]
    fn recall_kind_roundtrips() {
        assert_eq!(RecallKind::Group.as_str(), "group");
        assert_eq!(RecallKind::Friend.as_str(), "friend");
        assert_eq!(RecallKind::parse_from_str("group"), Some(RecallKind::Group));
        assert_eq!(
            RecallKind::parse_from_str("friend"),
            Some(RecallKind::Friend)
        );
        assert!(RecallKind::parse_from_str("invalid").is_none());
    }

    #[test]
    fn invalidation_target_all_covers_all_categories() {
        let all = InvalidationTarget::all();
        assert!(all.contains(&InvalidationTarget::ThreadSemanticCandidates));
        assert!(all.contains(&InvalidationTarget::OwnerResponseDraft));
        assert!(all.contains(&InvalidationTarget::PlannerTemporaryResults));
        assert!(all.contains(&InvalidationTarget::MemoryFactCandidates));
        assert!(all.contains(&InvalidationTarget::CommitmentFollowUpCandidates));
        assert!(all.contains(&InvalidationTarget::ArtifactReferences));
        assert_eq!(all.len(), 6);
    }
}
