//! 类型化动作白名单：Agent 只能选择白名单中的动作，不能构造任意
//! SQL、HTTP、Shell 或文件操作。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ContentTrustLevel, ConversationRef, MemoryFactId, MemoryPayload, NotificationCandidateRef,
    NotificationMatchKeyV1, NotificationOutcome, PolicyFamilyId, QuietHoursRule, SourceEventId,
};

use super::state::SecretaryAgentUpdate;
use super::validation::{SecretaryAgentRuntimeError, validate_action_proposal};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretaryRiskLevel {
    L0ReadOnly,
    L1Reversible,
    L2Impactful,
    L3ExternalSideEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretaryToolKind {
    SearchRecentEvents,
    ReadSourceEvent,
    SearchEventThreads,
    ResolveReference,
    ListUpcomingItems,
    GetSecretaryStatus,
    ListPendingOwnerWork,
    GetThreadContext,
    DraftReminder,
    CreateSchedule,
    RescheduleItem,
    CancelItem,
    CreateTask,
    CreateReminder,
    CompleteItem,
    SnoozeItem,
    SendOwnerMessage,
    AskOwnerClarification,
    ListNotificationPolicies,
    ExplainNotificationDecision,
    SetAccountDefaultNotificationMode,
    SetConversationNotificationMode,
    SetQuietHours,
    SetImportantContact,
    SetNotificationCategoryImportance,
    RecordNotificationFeedback,
    CreateSimilarNotificationRule,
    DisableNotificationPolicy,
    SetAutomaticReplyDeniedForContact,
    ListMemoryFacts,
    ReadMemoryFactSources,
    CorrectMemoryFact,
    DeleteMemoryFact,
    SetMemoryFactTtl,
    SetConversationMemoryMode,
    ConfirmThreadDecision,
    RevokeThreadDecision,
    DismissThreadQuestion,
    SetThreadLifecycle,
    DismissFollowUp,
    SnoozeFollowUp,
    DismissFollowUps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretaryToolPolicy {
    pub risk: SecretaryRiskLevel,
    pub requires_confirmation: bool,
    pub reversible: bool,
    pub timeout_ms: u64,
    pub max_retries: u8,
}

impl SecretaryToolKind {
    pub fn policy(self) -> SecretaryToolPolicy {
        use SecretaryRiskLevel::{L0ReadOnly, L1Reversible, L2Impactful, L3ExternalSideEffect};
        match self {
            Self::SearchRecentEvents
            | Self::ReadSourceEvent
            | Self::SearchEventThreads
            | Self::ResolveReference
            | Self::ListUpcomingItems
            | Self::GetSecretaryStatus
            | Self::ListPendingOwnerWork
            | Self::GetThreadContext
            | Self::ListNotificationPolicies
            | Self::ExplainNotificationDecision
            | Self::ListMemoryFacts
            | Self::ReadMemoryFactSources => SecretaryToolPolicy {
                risk: L0ReadOnly,
                requires_confirmation: false,
                reversible: true,
                timeout_ms: 10_000,
                max_retries: 2,
            },
            Self::DraftReminder | Self::RecordNotificationFeedback => SecretaryToolPolicy {
                risk: L1Reversible,
                requires_confirmation: false,
                reversible: true,
                timeout_ms: 5_000,
                max_retries: 1,
            },
            Self::CreateSchedule
            | Self::RescheduleItem
            | Self::CancelItem
            | Self::CreateTask
            | Self::CreateReminder
            | Self::CompleteItem
            | Self::SnoozeItem
            | Self::SetAccountDefaultNotificationMode
            | Self::SetConversationNotificationMode
            | Self::SetQuietHours
            | Self::SetImportantContact
            | Self::SetNotificationCategoryImportance
            | Self::CreateSimilarNotificationRule
            | Self::DisableNotificationPolicy
            | Self::SetAutomaticReplyDeniedForContact => SecretaryToolPolicy {
                risk: L2Impactful,
                requires_confirmation: true,
                reversible: true,
                timeout_ms: 15_000,
                max_retries: 1,
            },
            Self::CorrectMemoryFact
            | Self::DeleteMemoryFact
            | Self::SetMemoryFactTtl
            | Self::SetConversationMemoryMode
            | Self::ConfirmThreadDecision
            | Self::RevokeThreadDecision
            | Self::DismissThreadQuestion
            | Self::SetThreadLifecycle
            | Self::DismissFollowUp
            | Self::SnoozeFollowUp
            | Self::DismissFollowUps => SecretaryToolPolicy {
                risk: L2Impactful,
                requires_confirmation: true,
                reversible: true,
                timeout_ms: 15_000,
                max_retries: 1,
            },
            Self::SendOwnerMessage => SecretaryToolPolicy {
                risk: L3ExternalSideEffect,
                requires_confirmation: true,
                reversible: false,
                timeout_ms: 30_000,
                max_retries: 0,
            },
            Self::AskOwnerClarification => SecretaryToolPolicy {
                risk: L0ReadOnly,
                requires_confirmation: false,
                reversible: true,
                timeout_ms: 0,
                max_retries: 0,
            },
        }
    }
}

/// 批量忽略动作的单个目标；`expected_source_version` 必须来自
/// ListPendingOwnerWork 展示的 version N，落库时与行内 source_version CAS 比较。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUpControlTarget {
    pub follow_up_id: crate::FollowUpId,
    pub expected_source_version: u64,
}

/// Agent 只能选择白名单中的类型化动作，不能构造任意 SQL、HTTP、Shell 或文件操作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum SecretaryAction {
    SearchRecentEvents {
        query: String,
        limit: u16,
    },
    ReadSourceEvent {
        source_event_id: SourceEventId,
    },
    SearchEventThreads {
        query: String,
        limit: u16,
    },
    ResolveReference {
        expression: String,
    },
    ListUpcomingItems {
        horizon_secs: u64,
    },
    GetSecretaryStatus,
    ListPendingOwnerWork {
        limit: u16,
    },
    GetThreadContext {
        thread_id: crate::EventThreadId,
    },
    DraftReminder {
        text: String,
        due_at_unix: i64,
    },
    CreateSchedule {
        title: String,
        starts_at_unix: i64,
        timezone: String,
    },
    RescheduleItem {
        item_id: String,
        expected_version: u64,
        starts_at_unix: i64,
        timezone: String,
    },
    CancelItem {
        item_id: String,
        expected_version: u64,
        reason: String,
    },
    CreateTask {
        title: String,
        due_at_unix: Option<i64>,
        timezone: String,
    },
    CreateReminder {
        text: String,
        due_at_unix: i64,
        timezone: String,
    },
    CompleteItem {
        item_id: String,
        expected_version: u64,
    },
    SnoozeItem {
        item_id: String,
        expected_version: u64,
        due_at_unix: i64,
        timezone: String,
    },
    SendOwnerMessage {
        text: String,
    },
    AskOwnerClarification {
        question: String,
    },
    ListNotificationPolicies {
        limit: u16,
    },
    ExplainNotificationDecision {
        decision_id: String,
    },
    SetAccountDefaultNotificationMode {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        outcome: NotificationOutcome,
        bypass_quiet: bool,
    },
    SetConversationNotificationMode {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        outcome: NotificationOutcome,
        bypass_quiet: bool,
        fully_silent: bool,
        allow_bypass: bool,
    },
    SetQuietHours {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        quiet_hours: QuietHoursRule,
    },
    SetImportantContact {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        outcome: NotificationOutcome,
        bypass_quiet: bool,
    },
    SetNotificationCategoryImportance {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        outcome: NotificationOutcome,
        bypass_quiet: bool,
    },
    RecordNotificationFeedback {
        candidate: NotificationCandidateRef,
        match_key: NotificationMatchKeyV1,
        important: bool,
        promote_to_rule: bool,
    },
    CreateSimilarNotificationRule {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
        outcome: NotificationOutcome,
        bypass_quiet: bool,
    },
    DisableNotificationPolicy {
        policy_family_id: PolicyFamilyId,
        expected_generation: u64,
    },
    SetAutomaticReplyDeniedForContact {
        canonical_scope_key: String,
        match_key: NotificationMatchKeyV1,
    },
    ListMemoryFacts {
        limit: u16,
    },
    ReadMemoryFactSources {
        fact_id: MemoryFactId,
        max_excerpt_chars: u16,
    },
    CorrectMemoryFact {
        fact_id: MemoryFactId,
        replacement: MemoryPayload,
        confidence_bps: u16,
        source_event_ids: Vec<SourceEventId>,
        valid_until_unix_secs: Option<i64>,
    },
    DeleteMemoryFact {
        fact_id: MemoryFactId,
        reason: String,
    },
    SetMemoryFactTtl {
        fact_id: MemoryFactId,
        valid_until_unix_secs: Option<i64>,
    },
    SetConversationMemoryMode {
        conversation: ConversationRef,
        mode: ContentTrustLevel,
    },
    ConfirmThreadDecision {
        decision_id: crate::ThreadDecisionId,
    },
    RevokeThreadDecision {
        decision_id: crate::ThreadDecisionId,
        reason: String,
    },
    DismissThreadQuestion {
        question_id: crate::OpenQuestionId,
        reason: String,
    },
    SetThreadLifecycle {
        thread_id: crate::EventThreadId,
        expected_status: crate::ThreadStatus,
        target_status: crate::ThreadStatus,
        reason: String,
    },
    DismissFollowUp {
        follow_up_id: crate::FollowUpId,
        /// 审批时刻的期望来源版本（>= 1），落库时与行内 source_version CAS 比较。
        expected_source_version: u64,
        reason: String,
    },
    SnoozeFollowUp {
        follow_up_id: crate::FollowUpId,
        /// 审批时刻的期望来源版本（>= 1），落库时与行内 source_version CAS 比较。
        expected_source_version: u64,
        /// 新的到期时间（UTC Unix 秒）；执行时以数据库当前 UTC 时间复验，
        /// 必须晚于当前 due 且不超过数据库当前时间后 365 天。
        snooze_until_unix_secs: i64,
        reason: String,
    },
    /// 批量忽略（v1 只做 dismiss，不包含批量 Snooze/完成/模糊搜索）。
    /// targets 数量必须为 1..=20 且 follow_up_id 不重复；全有或全无，
    /// 任一目标校验失败则整个事务回滚。
    DismissFollowUps {
        targets: Vec<FollowUpControlTarget>,
        reason: String,
    },
}

impl SecretaryAction {
    pub fn kind(&self) -> SecretaryToolKind {
        match self {
            Self::SearchRecentEvents { .. } => SecretaryToolKind::SearchRecentEvents,
            Self::ReadSourceEvent { .. } => SecretaryToolKind::ReadSourceEvent,
            Self::SearchEventThreads { .. } => SecretaryToolKind::SearchEventThreads,
            Self::ResolveReference { .. } => SecretaryToolKind::ResolveReference,
            Self::ListUpcomingItems { .. } => SecretaryToolKind::ListUpcomingItems,
            Self::GetSecretaryStatus => SecretaryToolKind::GetSecretaryStatus,
            Self::ListPendingOwnerWork { .. } => SecretaryToolKind::ListPendingOwnerWork,
            Self::GetThreadContext { .. } => SecretaryToolKind::GetThreadContext,
            Self::DraftReminder { .. } => SecretaryToolKind::DraftReminder,
            Self::CreateSchedule { .. } => SecretaryToolKind::CreateSchedule,
            Self::RescheduleItem { .. } => SecretaryToolKind::RescheduleItem,
            Self::CancelItem { .. } => SecretaryToolKind::CancelItem,
            Self::CreateTask { .. } => SecretaryToolKind::CreateTask,
            Self::CreateReminder { .. } => SecretaryToolKind::CreateReminder,
            Self::CompleteItem { .. } => SecretaryToolKind::CompleteItem,
            Self::SnoozeItem { .. } => SecretaryToolKind::SnoozeItem,
            Self::SendOwnerMessage { .. } => SecretaryToolKind::SendOwnerMessage,
            Self::AskOwnerClarification { .. } => SecretaryToolKind::AskOwnerClarification,
            Self::ListNotificationPolicies { .. } => SecretaryToolKind::ListNotificationPolicies,
            Self::ExplainNotificationDecision { .. } => {
                SecretaryToolKind::ExplainNotificationDecision
            }
            Self::SetAccountDefaultNotificationMode { .. } => {
                SecretaryToolKind::SetAccountDefaultNotificationMode
            }
            Self::SetConversationNotificationMode { .. } => {
                SecretaryToolKind::SetConversationNotificationMode
            }
            Self::SetQuietHours { .. } => SecretaryToolKind::SetQuietHours,
            Self::SetImportantContact { .. } => SecretaryToolKind::SetImportantContact,
            Self::SetNotificationCategoryImportance { .. } => {
                SecretaryToolKind::SetNotificationCategoryImportance
            }
            Self::RecordNotificationFeedback { .. } => {
                SecretaryToolKind::RecordNotificationFeedback
            }
            Self::CreateSimilarNotificationRule { .. } => {
                SecretaryToolKind::CreateSimilarNotificationRule
            }
            Self::DisableNotificationPolicy { .. } => SecretaryToolKind::DisableNotificationPolicy,
            Self::SetAutomaticReplyDeniedForContact { .. } => {
                SecretaryToolKind::SetAutomaticReplyDeniedForContact
            }
            Self::ListMemoryFacts { .. } => SecretaryToolKind::ListMemoryFacts,
            Self::ReadMemoryFactSources { .. } => SecretaryToolKind::ReadMemoryFactSources,
            Self::CorrectMemoryFact { .. } => SecretaryToolKind::CorrectMemoryFact,
            Self::DeleteMemoryFact { .. } => SecretaryToolKind::DeleteMemoryFact,
            Self::SetMemoryFactTtl { .. } => SecretaryToolKind::SetMemoryFactTtl,
            Self::SetConversationMemoryMode { .. } => SecretaryToolKind::SetConversationMemoryMode,
            Self::ConfirmThreadDecision { .. } => SecretaryToolKind::ConfirmThreadDecision,
            Self::RevokeThreadDecision { .. } => SecretaryToolKind::RevokeThreadDecision,
            Self::DismissThreadQuestion { .. } => SecretaryToolKind::DismissThreadQuestion,
            Self::SetThreadLifecycle { .. } => SecretaryToolKind::SetThreadLifecycle,
            Self::DismissFollowUp { .. } => SecretaryToolKind::DismissFollowUp,
            Self::SnoozeFollowUp { .. } => SecretaryToolKind::SnoozeFollowUp,
            Self::DismissFollowUps { .. } => SecretaryToolKind::DismissFollowUps,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionProposal {
    pub proposal_id: String,
    pub action: SecretaryAction,
    pub rationale: String,
    pub source_event_ids: Vec<SourceEventId>,
    pub idempotency_key: Option<String>,
}

impl SecretaryActionProposal {
    pub fn new(
        action: SecretaryAction,
        rationale: impl Into<String>,
        source_event_ids: Vec<SourceEventId>,
        idempotency_key: Option<String>,
    ) -> Result<Self, SecretaryAgentRuntimeError> {
        let proposal = Self {
            proposal_id: Uuid::new_v4().to_string(),
            action,
            rationale: rationale.into(),
            source_event_ids,
            idempotency_key,
        };
        validate_action_proposal(&proposal)?;
        Ok(proposal)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionEffect {
    pub proposal: SecretaryActionProposal,
}

impl agent_core::graph::AgentEffect for SecretaryActionEffect {
    type Update = SecretaryAgentUpdate;
    type Receipt = SecretaryActionReceipt;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<agent_core::AgentUpdate<Self::Update>> {
        vec![agent_core::AgentUpdate::Business(
            SecretaryAgentUpdate::ActionCompleted(receipt.clone()),
        )]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionReceipt {
    pub proposal_id: String,
    pub result_ref: String,
    /// 产生此回执的 Action 类型；通知策略 Action 通过此字段区分响应工件解析方式。
    #[serde(default)]
    pub tool_kind: Option<SecretaryToolKind>,
}
