//! Agent 工作状态、Phase、Update 与 `AgentBusinessState` 实现。
//!
//! 有界工作状态：原始消息正文和完整工具结果始终留在外部事件日志中。
//! 本模块只持有指针与有界摘要。

use agent_core::{AgentBusinessState, AgentStateError, AgentUpdate};
use serde::{Deserialize, Serialize};

use crate::SourceEventId;

use super::action::{SecretaryActionEffect, SecretaryActionProposal, SecretaryActionReceipt};
use super::approval::SecretaryApprovalDecision;
use super::response::{OwnerResponseDraft, RecentEventRef};
use super::validation::{
    SecretaryAgentRuntimeError, validate_action_proposal, validate_agent_state,
    validate_response_draft,
};

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

/// `SecretaryActionProposal` 是公共协议类型，对其 Boxing 会破坏所有调用方与序列化兼容性，
/// 因此在没有独立兼容迁移方案之前抑制该枚举尺寸警告。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretaryAgentUpdate {
    ProposalAccepted(SecretaryActionProposal),
    ApprovalResolved(super::approval::SecretaryActionResumeInput),
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
    type SuspendData = super::approval::SecretaryActionApprovalRequest;
    type ResumeInput = super::approval::SecretaryActionResumeInput;

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
