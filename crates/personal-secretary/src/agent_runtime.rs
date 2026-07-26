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
const MAX_RESPONSE_SEGMENTS: usize = 20;
const MAX_SEGMENT_CHARS: usize = 1_000;
const MAX_DRAFT_TOTAL_CHARS: usize = 8_000;

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

/// Owner 响应草稿的单个片段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseSegment {
    /// 来自检索结果的有界正文摘录。envelope_only 内容此处为空字符串。
    Excerpt {
        source_event_id: SourceEventId,
        text: String,
    },
    /// Planner 生成的自然语言摘要。
    Summary { text: String },
}

impl ResponseSegment {
    /// 单条片段正文的字符数。
    fn char_count(&self) -> usize {
        match self {
            Self::Excerpt { text, .. } | Self::Summary { text } => text.chars().count(),
        }
    }

    /// 该片段引用的 source_event_id（Summary 无）。
    pub fn source_event_id(&self) -> Option<&SourceEventId> {
        match self {
            Self::Excerpt {
                source_event_id, ..
            } => Some(source_event_id),
            Self::Summary { .. } => None,
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Self::Excerpt { text, .. } | Self::Summary { text } => text,
        }
    }
}

/// Owner 收到的响应草稿。正文有界，来源失效时可标记失效。
///
/// 约束 7：只保存有界摘录；限制单条/总字符数；序列化字节数（64KB）由应用层验证。
/// 来源删除/过期/不可见时调用 `invalidate_if_references` 标记失效，由上层重新脱敏或重建。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerResponseDraft {
    segments: Vec<ResponseSegment>,
    /// 草稿依据的来源事件 ID（含 excerpts 引用 + 额外 evidence）。
    source_event_ids: Vec<SourceEventId>,
    created_at_unix_secs: i64,
    /// 是否已因来源失效而标记失效。私有，只能通过 `invalidate_if_references` 修改。
    invalidated: bool,
}

impl OwnerResponseDraft {
    pub fn new(
        segments: Vec<ResponseSegment>,
        source_event_ids: Vec<SourceEventId>,
        created_at_unix_secs: i64,
    ) -> Result<Self, SecretaryAgentRuntimeError> {
        let draft = Self {
            segments,
            source_event_ids,
            created_at_unix_secs,
            invalidated: false,
        };
        validate_response_draft(&draft)?;
        Ok(draft)
    }

    pub fn segments(&self) -> &[ResponseSegment] {
        &self.segments
    }

    pub fn source_event_ids(&self) -> &[SourceEventId] {
        &self.source_event_ids
    }

    pub fn created_at_unix_secs(&self) -> i64 {
        self.created_at_unix_secs
    }

    pub fn invalidated(&self) -> bool {
        self.invalidated
    }

    /// 检查草稿是否引用了已移除的来源事件，若是则标记失效。
    /// 返回是否发生了失效转换（已失效时再次调用返回 false）。
    pub fn invalidate_if_references(&mut self, removed_event_ids: &[SourceEventId]) -> bool {
        if self.invalidated {
            return false;
        }
        let removed: HashSet<&str> = removed_event_ids
            .iter()
            .map(SourceEventId::as_str)
            .collect();
        let references_removed = self
            .source_event_ids
            .iter()
            .any(|id| removed.contains(id.as_str()))
            || self.segments.iter().any(|seg| {
                seg.source_event_id()
                    .is_some_and(|id| removed.contains(id.as_str()))
            });
        if references_removed {
            self.invalidated = true;
            return true;
        }
        false
    }

    /// 草稿正文总字符数（所有 segments 之和）。
    pub fn total_char_count(&self) -> usize {
        self.segments.iter().map(|s| s.char_count()).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretaryAgentUpdate {
    ProposalAccepted(SecretaryActionProposal),
    ApprovalResolved(SecretaryActionResumeInput),
    ActionCompleted(SecretaryActionReceipt),
    /// UpdateState 节点构建好响应草稿后发送，将 phase 置为 Respond。
    ResponseReady(OwnerResponseDraft),
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
    #[serde(default)]
    response_draft: Option<OwnerResponseDraft>,
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
            response_draft: None,
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

    pub fn last_receipt(&self) -> Option<&SecretaryActionReceipt> {
        self.last_receipt.as_ref()
    }

    pub fn response_draft(&self) -> Option<&OwnerResponseDraft> {
        self.response_draft.as_ref()
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
            SecretaryAgentUpdate::ResponseReady(draft) => {
                validate_response_draft(&draft)
                    .map_err(|error| AgentStateError::Business(error.to_string()))?;
                self.response_draft = Some(draft);
                self.phase = SecretaryAgentPhase::Respond;
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

/// 校验 Owner 响应草稿的有界约束（约束 7）。
///
/// - segments 数量上限 `MAX_RESPONSE_SEGMENTS`
/// - 每条片段正文上限 `MAX_SEGMENT_CHARS`
/// - 全部片段总字符数上限 `MAX_DRAFT_TOTAL_CHARS`
/// - source_event_ids 数量上限 `MAX_EVIDENCE`，且必须唯一
/// - created_at_unix_secs 非负
///
/// 序列化字节数的 64KB 限制由应用层在持久化时验证。
pub fn validate_response_draft(
    draft: &OwnerResponseDraft,
) -> Result<(), SecretaryAgentRuntimeError> {
    if draft.segments.is_empty() {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(
            "response draft must contain at least one segment".into(),
        ));
    }
    if draft.segments.len() > MAX_RESPONSE_SEGMENTS {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(format!(
            "response draft must not exceed {MAX_RESPONSE_SEGMENTS} segments"
        )));
    }
    for segment in &draft.segments {
        let text = segment.text();
        let count = text.chars().count();
        if count > MAX_SEGMENT_CHARS {
            return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(format!(
                "segment text must not exceed {MAX_SEGMENT_CHARS} characters"
            )));
        }
        if let ResponseSegment::Excerpt { text, .. } = segment
            && text.is_empty()
        {
            // envelope_only 摘录允许空文本；但 Summary 必须非空。
            continue;
        }
        if segment.text().trim().is_empty() {
            return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(
                "segment text must not be blank".into(),
            ));
        }
    }
    let total = draft.total_char_count();
    if total > MAX_DRAFT_TOTAL_CHARS {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(format!(
            "response draft total characters must not exceed {MAX_DRAFT_TOTAL_CHARS}"
        )));
    }
    if draft.source_event_ids.len() > MAX_EVIDENCE {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(format!(
            "response draft source_event_ids must not exceed {MAX_EVIDENCE} items"
        )));
    }
    let unique = draft
        .source_event_ids
        .iter()
        .map(SourceEventId::as_str)
        .collect::<HashSet<_>>();
    if unique.len() != draft.source_event_ids.len() {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(
            "response draft source_event_ids must be unique".into(),
        ));
    }
    if draft.created_at_unix_secs < 0 {
        return Err(SecretaryAgentRuntimeError::InvalidResponseDraft(
            "created_at_unix_secs must not be negative".into(),
        ));
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
    #[error("invalid owner response draft: {0}")]
    InvalidResponseDraft(String),
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

    fn excerpt_segment(event_id: &str, text: &str) -> ResponseSegment {
        ResponseSegment::Excerpt {
            source_event_id: SourceEventId::new(event_id).unwrap(),
            text: text.into(),
        }
    }

    fn summary_segment(text: impl Into<String>) -> ResponseSegment {
        ResponseSegment::Summary { text: text.into() }
    }

    fn draft(segments: Vec<ResponseSegment>, source_event_ids: Vec<&str>) -> OwnerResponseDraft {
        OwnerResponseDraft::new(
            segments,
            source_event_ids
                .into_iter()
                .map(|id| SourceEventId::new(id).unwrap())
                .collect(),
            1_000,
        )
        .unwrap()
    }

    #[test]
    fn response_draft_accepts_bounded_segments() {
        let d = draft(
            vec![
                excerpt_segment("event-1", "老板说：明天开会"),
                summary_segment("建议明天 10 点提醒"),
            ],
            vec!["event-1"],
        );
        assert_eq!(d.segments().len(), 2);
        assert!(!d.invalidated());
    }

    #[test]
    fn response_draft_rejects_empty_segments() {
        let error = OwnerResponseDraft::new(Vec::new(), Vec::new(), 1_000).unwrap_err();
        assert!(matches!(
            error,
            SecretaryAgentRuntimeError::InvalidResponseDraft(_)
        ));
    }

    #[test]
    fn response_draft_rejects_too_many_segments() {
        let segments: Vec<ResponseSegment> = (0..=MAX_RESPONSE_SEGMENTS)
            .map(|i| summary_segment(format!("段 {i}")))
            .collect();
        let error = OwnerResponseDraft::new(segments, Vec::new(), 1_000).unwrap_err();
        assert!(matches!(
            error,
            SecretaryAgentRuntimeError::InvalidResponseDraft(_)
        ));
    }

    #[test]
    fn response_draft_rejects_oversized_segment() {
        let segment = summary_segment("x".repeat(MAX_SEGMENT_CHARS + 1));
        let error = OwnerResponseDraft::new(vec![segment], Vec::new(), 1_000).unwrap_err();
        assert!(matches!(
            error,
            SecretaryAgentRuntimeError::InvalidResponseDraft(_)
        ));
    }

    #[test]
    fn response_draft_rejects_blank_summary() {
        let segment = summary_segment("   ");
        let error = OwnerResponseDraft::new(vec![segment], Vec::new(), 1_000).unwrap_err();
        assert!(matches!(
            error,
            SecretaryAgentRuntimeError::InvalidResponseDraft(_)
        ));
    }

    #[test]
    fn response_draft_allows_empty_excerpt_text_for_envelope_only() {
        let segment = excerpt_segment("event-1", "");
        let d = OwnerResponseDraft::new(vec![segment], vec![], 1_000).unwrap();
        assert_eq!(d.segments().len(), 1);
    }

    #[test]
    fn response_draft_rejects_total_chars_exceeding_limit() {
        // 每条 MAX_SEGMENT_CHARS，总字符数超过 MAX_DRAFT_TOTAL_CHARS
        let count = (MAX_DRAFT_TOTAL_CHARS / MAX_SEGMENT_CHARS) + 2;
        let segments: Vec<ResponseSegment> = (0..count)
            .map(|_| summary_segment("x".repeat(MAX_SEGMENT_CHARS)))
            .collect();
        let error = OwnerResponseDraft::new(segments, Vec::new(), 1_000).unwrap_err();
        assert!(matches!(
            error,
            SecretaryAgentRuntimeError::InvalidResponseDraft(_)
        ));
    }

    #[test]
    fn response_draft_rejects_negative_created_at() {
        let segment = summary_segment("有效摘要");
        let error = OwnerResponseDraft::new(vec![segment], Vec::new(), -1).unwrap_err();
        assert!(matches!(
            error,
            SecretaryAgentRuntimeError::InvalidResponseDraft(_)
        ));
    }

    #[test]
    fn response_draft_rejects_duplicate_source_event_ids() {
        let segment = excerpt_segment("event-1", "内容");
        let error = OwnerResponseDraft::new(
            vec![segment],
            vec![
                SourceEventId::new("event-1").unwrap(),
                SourceEventId::new("event-1").unwrap(),
            ],
            1_000,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            SecretaryAgentRuntimeError::InvalidResponseDraft(_)
        ));
    }

    #[test]
    fn response_draft_rejects_too_many_source_event_ids() {
        let segment = summary_segment("摘要");
        let ids: Vec<SourceEventId> = (0..=MAX_EVIDENCE)
            .map(|i| SourceEventId::new(format!("event-{i}")).unwrap())
            .collect();
        let error = OwnerResponseDraft::new(vec![segment], ids, 1_000).unwrap_err();
        assert!(matches!(
            error,
            SecretaryAgentRuntimeError::InvalidResponseDraft(_)
        ));
    }

    #[test]
    fn invalidate_marks_draft_when_excerpt_source_removed() {
        let mut d = draft(
            vec![excerpt_segment("event-1", "老板说：明天开会")],
            vec!["event-1"],
        );
        assert!(d.invalidate_if_references(&[SourceEventId::new("event-1").unwrap()]));
        assert!(d.invalidated());
    }

    #[test]
    fn invalidate_marks_draft_when_evidence_source_removed() {
        let mut d = draft(vec![summary_segment("摘要")], vec!["event-1", "event-2"]);
        // 摘要段未直接引用，但 source_event_ids 含 event-2
        assert!(d.invalidate_if_references(&[SourceEventId::new("event-2").unwrap()]));
        assert!(d.invalidated());
    }

    #[test]
    fn invalidate_does_not_mark_draft_when_source_not_referenced() {
        let mut d = draft(vec![excerpt_segment("event-1", "内容")], vec!["event-1"]);
        assert!(!d.invalidate_if_references(&[SourceEventId::new("event-99").unwrap()]));
        assert!(!d.invalidated());
    }

    #[test]
    fn invalidate_is_idempotent() {
        let mut d = draft(vec![excerpt_segment("event-1", "内容")], vec!["event-1"]);
        assert!(d.invalidate_if_references(&[SourceEventId::new("event-1").unwrap()]));
        // 已失效后再次调用返回 false
        assert!(!d.invalidate_if_references(&[SourceEventId::new("event-1").unwrap()]));
    }

    #[test]
    fn response_ready_update_sets_phase_to_respond() {
        let mut state = SecretaryAgentState::new(
            "处理日程",
            Vec::new(),
            vec![SourceEventId::new("event-1").unwrap()],
            Vec::new(),
        )
        .unwrap();
        assert_eq!(state.phase(), SecretaryAgentPhase::Observe);

        let d = draft(vec![summary_segment("已处理")], vec!["event-1"]);
        state
            .apply_update(SecretaryAgentUpdate::ResponseReady(d.clone()))
            .unwrap();
        assert_eq!(state.phase(), SecretaryAgentPhase::Respond);
        assert_eq!(state.response_draft(), Some(&d));
    }

    #[test]
    fn response_ready_update_rejects_invalid_draft() {
        let mut state =
            SecretaryAgentState::new("处理日程", Vec::new(), Vec::new(), Vec::new()).unwrap();
        // 通过反序列化绕过 `new` 校验，构造一个 segments 为空的非法 draft，
        // 验证 `apply_update` 仍会拒绝并保持 phase 不变。
        let invalid_draft: OwnerResponseDraft = serde_json::from_value(serde_json::json!({
            "segments": [],
            "source_event_ids": [],
            "created_at_unix_secs": 1000,
            "invalidated": false
        }))
        .unwrap();
        let error = state
            .apply_update(SecretaryAgentUpdate::ResponseReady(invalid_draft))
            .unwrap_err();
        assert!(matches!(error, AgentStateError::Business(_)));
        assert_eq!(state.phase(), SecretaryAgentPhase::Observe);
        assert!(state.response_draft().is_none());
    }

    #[test]
    fn agent_state_serialization_remains_backward_compatible() {
        // 反序列化旧状态（无 response_draft 字段）应成功并默认为 None。
        let json = serde_json::json!({
            "goal": "处理日程",
            "phase": "observe",
            "invariants": [],
            "evidence_source_event_ids": [],
            "recent_events": [],
            "pending_proposal": null,
            "last_receipt": null
        });
        let state: SecretaryAgentState = serde_json::from_value(json).unwrap();
        assert!(state.response_draft().is_none());
    }
}
