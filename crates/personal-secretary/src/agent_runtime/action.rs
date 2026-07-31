//! 类型化动作白名单：Agent 只能选择白名单中的动作，不能构造任意
//! SQL、HTTP、Shell 或文件操作。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    NotificationCandidateRef, NotificationMatchKeyV1, NotificationOutcome, PolicyFamilyId,
    QuietHoursRule, SourceEventId,
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
            | Self::ListNotificationPolicies
            | Self::ExplainNotificationDecision => SecretaryToolPolicy {
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
}

impl SecretaryAction {
    pub fn kind(&self) -> SecretaryToolKind {
        match self {
            Self::SearchRecentEvents { .. } => SecretaryToolKind::SearchRecentEvents,
            Self::ReadSourceEvent { .. } => SecretaryToolKind::ReadSourceEvent,
            Self::SearchEventThreads { .. } => SecretaryToolKind::SearchEventThreads,
            Self::ResolveReference { .. } => SecretaryToolKind::ResolveReference,
            Self::ListUpcomingItems { .. } => SecretaryToolKind::ListUpcomingItems,
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
}
