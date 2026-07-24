use std::collections::HashSet;

use agent_core::graph::{NodeResult, SuspendReason, SuspendRequest, UsageDelta};
use agent_core::{AgentBusinessState, AgentStateError, AgentUpdate};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::SourceEventId;

const MAX_GOAL_CHARS: usize = 2_000;
const MAX_INVARIANTS: usize = 50;
const MAX_EVIDENCE: usize = 100;
const MAX_RECENT_EVENTS: usize = 8;
const MAX_TEXT_CHARS: usize = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretaryAgentPhase {
    Observe,
    Plan,
    Retrieve,
    ProposeAction,
    Validate,
    Execute,
    Suspended,
    UpdateState,
    Respond,
    Completed,
}

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
    SendOwnerMessage,
    AskOwnerClarification,
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
            | Self::ListUpcomingItems => SecretaryToolPolicy {
                risk: L0ReadOnly,
                requires_confirmation: false,
                reversible: true,
                timeout_ms: 10_000,
                max_retries: 2,
            },
            Self::DraftReminder => SecretaryToolPolicy {
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
            | Self::CreateReminder => SecretaryToolPolicy {
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
    },
    RescheduleItem {
        item_id: String,
        starts_at_unix: i64,
    },
    CancelItem {
        item_id: String,
        reason: String,
    },
    CreateTask {
        title: String,
        due_at_unix: Option<i64>,
    },
    CreateReminder {
        text: String,
        due_at_unix: i64,
    },
    SendOwnerMessage {
        text: String,
    },
    AskOwnerClarification {
        question: String,
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
            Self::SendOwnerMessage { .. } => SecretaryToolKind::SendOwnerMessage,
            Self::AskOwnerClarification { .. } => SecretaryToolKind::AskOwnerClarification,
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
pub struct RecentEventRef {
    pub source_event_id: SourceEventId,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionApprovalRequest {
    pub proposal_id: String,
    pub tool: SecretaryToolKind,
    pub risk: SecretaryRiskLevel,
    pub summary: String,
    pub source_event_ids: Vec<SourceEventId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretaryApprovalDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionResumeInput {
    pub proposal_id: String,
    pub decision: SecretaryApprovalDecision,
    pub command_source_event_id: SourceEventId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionEffect {
    pub proposal: SecretaryActionProposal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryActionReceipt {
    pub proposal_id: String,
    pub result_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretaryAgentUpdate {
    ProposalAccepted(SecretaryActionProposal),
    ApprovalResolved(SecretaryActionResumeInput),
    ActionCompleted(SecretaryActionReceipt),
    PhaseChanged(SecretaryAgentPhase),
}

/// 有界工作状态。原始消息正文和完整工具结果始终留在外部事件日志中。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretaryAgentState {
    goal: String,
    phase: SecretaryAgentPhase,
    invariants: Vec<String>,
    evidence_source_event_ids: Vec<SourceEventId>,
    recent_events: Vec<RecentEventRef>,
    pending_proposal: Option<SecretaryActionProposal>,
    last_receipt: Option<SecretaryActionReceipt>,
}

impl SecretaryAgentState {
    pub fn new(
        goal: impl Into<String>,
        invariants: Vec<String>,
        evidence_source_event_ids: Vec<SourceEventId>,
        recent_events: Vec<RecentEventRef>,
    ) -> Result<Self, SecretaryAgentRuntimeError> {
        let state = Self {
            goal: goal.into(),
            phase: SecretaryAgentPhase::Observe,
            invariants,
            evidence_source_event_ids,
            recent_events,
            pending_proposal: None,
            last_receipt: None,
        };
        validate_agent_state(&state)?;
        Ok(state)
    }

    pub fn goal(&self) -> &str {
        &self.goal
    }

    pub fn phase(&self) -> SecretaryAgentPhase {
        self.phase
    }

    pub fn invariants(&self) -> &[String] {
        &self.invariants
    }

    pub fn evidence_source_event_ids(&self) -> &[SourceEventId] {
        &self.evidence_source_event_ids
    }

    pub fn recent_events(&self) -> &[RecentEventRef] {
        &self.recent_events
    }

    pub fn pending_proposal(&self) -> Option<&SecretaryActionProposal> {
        self.pending_proposal.as_ref()
    }
}

impl AgentBusinessState for SecretaryAgentState {
    type Update = SecretaryAgentUpdate;
    type Effect = SecretaryActionEffect;
    type SuspendData = SecretaryActionApprovalRequest;
    type ResumeInput = SecretaryActionResumeInput;

    fn resume_updates(input: Self::ResumeInput) -> Vec<AgentUpdate<Self::Update>> {
        vec![AgentUpdate::Business(
            SecretaryAgentUpdate::ApprovalResolved(input),
        )]
    }

    fn apply_update(&mut self, update: Self::Update) -> Result<(), AgentStateError> {
        match update {
            SecretaryAgentUpdate::ProposalAccepted(proposal) => {
                validate_action_proposal(&proposal)
                    .map_err(|error| AgentStateError::Business(error.to_string()))?;
                self.pending_proposal = Some(proposal);
                self.phase = SecretaryAgentPhase::Execute;
            }
            SecretaryAgentUpdate::ApprovalResolved(input) => {
                let pending = self
                    .pending_proposal
                    .as_ref()
                    .ok_or_else(|| AgentStateError::Business("没有可恢复的待确认动作".into()))?;
                if pending.proposal_id != input.proposal_id {
                    return Err(AgentStateError::Business(
                        "恢复输入与待确认动作不匹配".into(),
                    ));
                }
                self.phase = match input.decision {
                    SecretaryApprovalDecision::Approve => SecretaryAgentPhase::Execute,
                    SecretaryApprovalDecision::Reject => SecretaryAgentPhase::Respond,
                };
            }
            SecretaryAgentUpdate::ActionCompleted(receipt) => {
                let pending = self.pending_proposal.as_ref().ok_or_else(|| {
                    AgentStateError::Business("动作回执缺少待执行 Proposal".into())
                })?;
                if pending.proposal_id != receipt.proposal_id {
                    return Err(AgentStateError::Business(
                        "动作回执与待执行 Proposal 不匹配".into(),
                    ));
                }
                self.last_receipt = Some(receipt);
                self.pending_proposal = None;
                self.phase = SecretaryAgentPhase::UpdateState;
            }
            SecretaryAgentUpdate::PhaseChanged(phase) => self.phase = phase,
        }
        Ok(())
    }
}

/// 策略门是唯一把模型 Proposal 转为 Effect/Suspend 的入口。
pub fn gate_secretary_action(
    proposal: SecretaryActionProposal,
) -> Result<
    NodeResult<SecretaryAgentUpdate, SecretaryActionEffect, SecretaryActionApprovalRequest>,
    SecretaryAgentRuntimeError,
> {
    validate_action_proposal(&proposal)?;
    let policy = proposal.action.kind().policy();
    let update = AgentUpdate::Business(SecretaryAgentUpdate::ProposalAccepted(proposal.clone()));

    if matches!(
        proposal.action,
        SecretaryAction::AskOwnerClarification { .. }
    ) {
        return Ok(NodeResult::suspend(
            vec![update],
            Vec::new(),
            UsageDelta::default(),
            SuspendRequest::new(
                SuspendReason::ExternalInput,
                approval_request(&proposal, policy),
            ),
        ));
    }
    if policy.requires_confirmation {
        return Ok(NodeResult::suspend(
            vec![update],
            Vec::new(),
            UsageDelta::default(),
            SuspendRequest::new(SuspendReason::Approval, approval_request(&proposal, policy)),
        ));
    }
    Ok(NodeResult::with_effect(
        vec![update],
        SecretaryActionEffect { proposal },
        UsageDelta::default(),
    ))
}

fn approval_request(
    proposal: &SecretaryActionProposal,
    policy: SecretaryToolPolicy,
) -> SecretaryActionApprovalRequest {
    SecretaryActionApprovalRequest {
        proposal_id: proposal.proposal_id.clone(),
        tool: proposal.action.kind(),
        risk: policy.risk,
        summary: proposal.rationale.clone(),
        source_event_ids: proposal.source_event_ids.clone(),
    }
}

pub fn validate_action_proposal(
    proposal: &SecretaryActionProposal,
) -> Result<(), SecretaryAgentRuntimeError> {
    if proposal.proposal_id.trim().is_empty() || proposal.proposal_id.len() > 191 {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(
            "proposal_id must contain 1..=191 bytes".into(),
        ));
    }
    bounded_text("rationale", &proposal.rationale, 1, 1_000)?;
    if proposal.source_event_ids.len() > 20 {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(
            "a proposal may reference at most 20 source events".into(),
        ));
    }
    let unique = proposal
        .source_event_ids
        .iter()
        .map(SourceEventId::as_str)
        .collect::<HashSet<_>>();
    if unique.len() != proposal.source_event_ids.len() {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(
            "proposal source events must be unique".into(),
        ));
    }
    validate_action(&proposal.action)?;
    let policy = proposal.action.kind().policy();
    if policy.risk >= SecretaryRiskLevel::L2Impactful
        && proposal
            .idempotency_key
            .as_deref()
            .is_none_or(|key| key.trim().is_empty() || key.len() > 191)
    {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(
            "L2/L3 actions require a bounded idempotency_key".into(),
        ));
    }
    Ok(())
}

fn validate_action(action: &SecretaryAction) -> Result<(), SecretaryAgentRuntimeError> {
    match action {
        SecretaryAction::SearchRecentEvents { query, limit }
        | SecretaryAction::SearchEventThreads { query, limit } => {
            bounded_text("query", query, 1, 1_000)?;
            if !(1..=100).contains(limit) {
                return Err(SecretaryAgentRuntimeError::InvalidProposal(
                    "search limit must be in 1..=100".into(),
                ));
            }
        }
        SecretaryAction::ResolveReference { expression } => {
            bounded_text("expression", expression, 1, 1_000)?;
        }
        SecretaryAction::ListUpcomingItems { horizon_secs } if *horizon_secs == 0 => {
            return Err(SecretaryAgentRuntimeError::InvalidProposal(
                "horizon_secs must be positive".into(),
            ));
        }
        SecretaryAction::DraftReminder { text, .. }
        | SecretaryAction::CreateReminder { text, .. }
        | SecretaryAction::SendOwnerMessage { text } => {
            bounded_text("text", text, 1, MAX_TEXT_CHARS)?;
        }
        SecretaryAction::CreateSchedule { title, .. }
        | SecretaryAction::CreateTask { title, .. } => {
            bounded_text("title", title, 1, 500)?;
        }
        SecretaryAction::RescheduleItem { item_id, .. } => {
            bounded_text("item_id", item_id, 1, 191)?;
        }
        SecretaryAction::CancelItem { item_id, reason } => {
            bounded_text("item_id", item_id, 1, 191)?;
            bounded_text("reason", reason, 1, 1_000)?;
        }
        SecretaryAction::AskOwnerClarification { question } => {
            bounded_text("question", question, 1, 1_000)?;
        }
        SecretaryAction::ReadSourceEvent { .. } | SecretaryAction::ListUpcomingItems { .. } => {}
    }
    Ok(())
}

fn validate_agent_state(state: &SecretaryAgentState) -> Result<(), SecretaryAgentRuntimeError> {
    bounded_text("goal", &state.goal, 1, MAX_GOAL_CHARS)?;
    if state.invariants.len() > MAX_INVARIANTS
        || state.evidence_source_event_ids.len() > MAX_EVIDENCE
        || state.recent_events.len() > MAX_RECENT_EVENTS
    {
        return Err(SecretaryAgentRuntimeError::InvalidState(
            "agent state exceeds bounded context limits".into(),
        ));
    }
    for invariant in &state.invariants {
        bounded_text("invariant", invariant, 1, 1_000)?;
    }
    for recent in &state.recent_events {
        bounded_text("recent event summary", &recent.summary, 1, 500)?;
    }
    Ok(())
}

fn bounded_text(
    field: &str,
    value: &str,
    min: usize,
    max: usize,
) -> Result<(), SecretaryAgentRuntimeError> {
    let count = value.chars().count();
    if !(min..=max).contains(&count) {
        return Err(SecretaryAgentRuntimeError::InvalidProposal(format!(
            "{field} must contain {min}..={max} characters"
        )));
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SecretaryAgentRuntimeError {
    #[error("invalid secretary action proposal: {0}")]
    InvalidProposal(String),
    #[error("invalid secretary agent state: {0}")]
    InvalidState(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(action: SecretaryAction, key: Option<&str>) -> SecretaryActionProposal {
        SecretaryActionProposal::new(
            action,
            "根据已确认事件执行",
            vec![SourceEventId::new("event-1").unwrap()],
            key.map(str::to_owned),
        )
        .unwrap()
    }

    #[test]
    fn read_only_action_executes_without_suspension() {
        let result = gate_secretary_action(proposal(
            SecretaryAction::SearchRecentEvents {
                query: "老板今天找过我吗".into(),
                limit: 20,
            },
            None,
        ))
        .unwrap();
        assert!(matches!(result, NodeResult::Continue { .. }));
        assert_eq!(result.effects().len(), 1);
    }

    #[test]
    fn external_side_effect_always_suspends_without_effect() {
        let result = gate_secretary_action(proposal(
            SecretaryAction::SendOwnerMessage {
                text: "报价单还有两小时截止，是否提醒负责人？".into(),
            },
            Some("notify:quote:deadline"),
        ))
        .unwrap();
        match result {
            NodeResult::Suspend {
                request, effects, ..
            } => {
                assert_eq!(request.reason, SuspendReason::Approval);
                assert_eq!(request.data.risk, SecretaryRiskLevel::L3ExternalSideEffect);
                assert!(effects.is_empty());
            }
            NodeResult::Continue { .. } => panic!("L3 action must suspend"),
        }
    }

    #[test]
    fn impactful_action_without_idempotency_key_is_rejected() {
        let error = SecretaryActionProposal::new(
            SecretaryAction::CreateReminder {
                text: "提交报价单".into(),
                due_at_unix: 1_800_000_000,
            },
            "用户要求创建提醒",
            Vec::new(),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("idempotency_key"));
    }

    #[test]
    fn working_state_rejects_unbounded_recent_window() {
        let events = (0..=MAX_RECENT_EVENTS)
            .map(|index| RecentEventRef {
                source_event_id: SourceEventId::new(format!("event-{index}")).unwrap(),
                summary: "精确来源摘要".into(),
            })
            .collect();
        let error =
            SecretaryAgentState::new("处理日程", Vec::new(), Vec::new(), events).unwrap_err();
        assert!(matches!(error, SecretaryAgentRuntimeError::InvalidState(_)));
    }

    #[test]
    fn tool_surface_has_no_arbitrary_sql_http_shell_or_filesystem_action() {
        let serialized = serde_json::to_string(&proposal(
            SecretaryAction::ListUpcomingItems {
                horizon_secs: 86_400,
            },
            None,
        ))
        .unwrap();
        for forbidden in ["sql", "http", "shell", "filesystem", "napcat_send"] {
            assert!(!serialized.contains(forbidden));
        }
    }
}
