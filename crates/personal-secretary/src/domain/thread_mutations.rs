use std::collections::HashSet;

use agent_core::graph::{AgentEffect, NodeResult, SuspendReason, SuspendRequest, UsageDelta};
use agent_core::{AgentBusinessState, AgentStateError, AgentUpdate};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{EventThreadId, SourceAccountRef, SourceEventId};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadMutationProposalId(String);

impl ThreadMutationProposalId {
    pub fn new(value: impl Into<String>) -> Result<Self, ThreadMutationError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ThreadMutationError::InvalidImpact(
                "proposal_id must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadMutationKind {
    Merge,
    Split,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadMutationDecision {
    Approve,
    Reject,
}

impl ThreadMutationDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadMutationProposalStatus {
    AwaitingApproval,
    Approved,
    Rejected,
    Applying,
    Applied,
    Failed,
    UnknownCommit,
}

impl ThreadMutationProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingApproval => "awaiting_approval",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Applying => "applying",
            Self::Applied => "applied",
            Self::Failed => "failed",
            Self::UnknownCommit => "unknown_commit",
        }
    }
}

impl ThreadMutationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Split => "split",
        }
    }
}

/// 高影响动作的纯数据预览。它不携带完整正文，只携带可回查 ID 和有界样本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMutationImpact {
    pub proposal_id: ThreadMutationProposalId,
    pub kind: ThreadMutationKind,
    pub account: SourceAccountRef,
    pub thread_ids: Vec<EventThreadId>,
    pub affected_event_count: u32,
    pub affected_conversation_count: u32,
    pub affected_source_event_ids: Vec<SourceEventId>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMutationApprovalRequest {
    pub proposal_id: ThreadMutationProposalId,
    pub kind: ThreadMutationKind,
    pub account: SourceAccountRef,
    pub thread_ids: Vec<EventThreadId>,
    pub affected_event_count: u32,
    pub affected_conversation_count: u32,
    pub affected_source_event_ids: Vec<SourceEventId>,
    pub warning: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMutationResumeInput {
    pub proposal_id: ThreadMutationProposalId,
    pub decision: ThreadMutationDecision,
    pub command_source_event_id: SourceEventId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMutationEffect {
    pub proposal_id: ThreadMutationProposalId,
    pub kind: ThreadMutationKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMutationEffectReceipt {
    pub proposal_id: ThreadMutationProposalId,
    pub effect_id: String,
    pub status: ThreadMutationProposalStatus,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMutationRevertInput {
    pub proposal_id: ThreadMutationProposalId,
    pub command_source_event_id: SourceEventId,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMutationRevertReceipt {
    pub proposal_id: ThreadMutationProposalId,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadMutationUpdate {
    OwnerDecision(ThreadMutationResumeInput),
    Rejected,
    Applied(ThreadMutationEffectReceipt),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMutationAgentState {
    impact: ThreadMutationImpact,
    resume_input: Option<ThreadMutationResumeInput>,
    status: ThreadMutationProposalStatus,
    receipt: Option<ThreadMutationEffectReceipt>,
}

impl ThreadMutationAgentState {
    pub fn new(impact: ThreadMutationImpact) -> Result<Self, ThreadMutationError> {
        validate_thread_mutation_impact(&impact)?;
        Ok(Self {
            impact,
            resume_input: None,
            status: ThreadMutationProposalStatus::AwaitingApproval,
            receipt: None,
        })
    }

    pub fn impact(&self) -> &ThreadMutationImpact {
        &self.impact
    }

    pub fn resume_input(&self) -> Option<&ThreadMutationResumeInput> {
        self.resume_input.as_ref()
    }

    pub fn status(&self) -> ThreadMutationProposalStatus {
        self.status
    }

    pub fn receipt(&self) -> Option<&ThreadMutationEffectReceipt> {
        self.receipt.as_ref()
    }
}

impl AgentBusinessState for ThreadMutationAgentState {
    type Update = ThreadMutationUpdate;
    type Effect = ThreadMutationEffect;
    type SuspendData = ThreadMutationApprovalRequest;
    type ResumeInput = ThreadMutationResumeInput;

    fn resume_updates(input: Self::ResumeInput) -> Vec<AgentUpdate<Self::Update>> {
        vec![AgentUpdate::Business(ThreadMutationUpdate::OwnerDecision(
            input,
        ))]
    }

    fn apply_update(&mut self, update: Self::Update) -> Result<(), AgentStateError> {
        match update {
            ThreadMutationUpdate::OwnerDecision(input) => {
                if input.proposal_id != self.impact.proposal_id || self.resume_input.is_some() {
                    return Err(AgentStateError::Business(
                        "线程变更 Resume 与待确认 Proposal 不匹配或已消费".into(),
                    ));
                }
                self.resume_input = Some(input);
            }
            ThreadMutationUpdate::Rejected => {
                self.status = ThreadMutationProposalStatus::Rejected;
            }
            ThreadMutationUpdate::Applied(receipt) => {
                if receipt.proposal_id != self.impact.proposal_id
                    || receipt.status != ThreadMutationProposalStatus::Applied
                {
                    return Err(AgentStateError::Business(
                        "线程变更 Effect Receipt 与 Proposal 不匹配".into(),
                    ));
                }
                self.status = ThreadMutationProposalStatus::Applied;
                self.receipt = Some(receipt);
            }
        }
        Ok(())
    }
}

impl AgentEffect for ThreadMutationEffect {
    type Update = ThreadMutationUpdate;
    type Receipt = ThreadMutationEffectReceipt;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>> {
        vec![AgentUpdate::Business(ThreadMutationUpdate::Applied(
            receipt.clone(),
        ))]
    }
}

pub fn validate_thread_mutation_impact(
    impact: &ThreadMutationImpact,
) -> Result<(), ThreadMutationError> {
    let thread_count = impact.thread_ids.len();
    match impact.kind {
        ThreadMutationKind::Merge if !(2..=10).contains(&thread_count) => {
            return Err(ThreadMutationError::InvalidImpact(
                "merge preview must contain 2..=10 threads".into(),
            ));
        }
        ThreadMutationKind::Split if thread_count != 1 => {
            return Err(ThreadMutationError::InvalidImpact(
                "split preview must contain exactly one source thread".into(),
            ));
        }
        _ => {}
    }
    if impact.affected_event_count == 0 || impact.affected_conversation_count == 0 {
        return Err(ThreadMutationError::InvalidImpact(
            "thread mutation must affect at least one event and conversation".into(),
        ));
    }
    if impact.affected_source_event_ids.is_empty() || impact.affected_source_event_ids.len() > 100 {
        return Err(ThreadMutationError::InvalidImpact(
            "impact preview must contain 1..=100 affected source events".into(),
        ));
    }
    if impact.affected_event_count as usize != impact.affected_source_event_ids.len() {
        return Err(ThreadMutationError::InvalidImpact(
            "affected_event_count must equal the exact affected source event id count".into(),
        ));
    }
    if impact.reason.trim().is_empty() || impact.reason.chars().count() > 1000 {
        return Err(ThreadMutationError::InvalidImpact(
            "impact reason must contain 1..=1000 characters".into(),
        ));
    }
    let unique_threads = impact
        .thread_ids
        .iter()
        .map(EventThreadId::as_str)
        .collect::<HashSet<_>>();
    let unique_events = impact
        .affected_source_event_ids
        .iter()
        .map(SourceEventId::as_str)
        .collect::<HashSet<_>>();
    if unique_threads.len() != impact.thread_ids.len()
        || unique_events.len() != impact.affected_source_event_ids.len()
    {
        return Err(ThreadMutationError::InvalidImpact(
            "impact preview contains duplicate thread or source event ids".into(),
        ));
    }
    Ok(())
}

pub fn validate_thread_mutation_revert(
    input: &ThreadMutationRevertInput,
) -> Result<(), ThreadMutationError> {
    if input.reason.trim().is_empty() || input.reason.chars().count() > 1000 {
        return Err(ThreadMutationError::InvalidRevert(
            "revert reason must contain 1..=1000 characters".into(),
        ));
    }
    Ok(())
}

/// 高影响线程操作只能从节点主动返回类型化 Suspend；此函数绝不产生执行 Effect。
pub fn suspend_thread_mutation_for_approval(
    impact: ThreadMutationImpact,
) -> Result<NodeResult<(), (), ThreadMutationApprovalRequest>, ThreadMutationError> {
    validate_thread_mutation_impact(&impact)?;
    let warning = match impact.kind {
        ThreadMutationKind::Merge => {
            "合并会改变后续检索与记忆投影；确认前不会移动任何事件".to_string()
        }
        ThreadMutationKind::Split => {
            "拆分会改变后续因果线程归属；确认前不会移动任何事件".to_string()
        }
    };
    Ok(NodeResult::Suspend {
        updates: Vec::new(),
        effects: Vec::new(),
        usage: UsageDelta::default(),
        request: SuspendRequest::new(
            SuspendReason::Approval,
            ThreadMutationApprovalRequest {
                proposal_id: impact.proposal_id,
                kind: impact.kind,
                account: impact.account,
                thread_ids: impact.thread_ids,
                affected_event_count: impact.affected_event_count,
                affected_conversation_count: impact.affected_conversation_count,
                affected_source_event_ids: impact.affected_source_event_ids,
                warning,
            },
        ),
    })
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ThreadMutationError {
    #[error("invalid thread mutation impact: {0}")]
    InvalidImpact(String),
    #[error("invalid thread mutation revert: {0}")]
    InvalidRevert(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessageSource, SourceAccountRef};

    fn impact(kind: ThreadMutationKind, thread_ids: &[&str]) -> ThreadMutationImpact {
        ThreadMutationImpact {
            proposal_id: ThreadMutationProposalId::new("proposal").unwrap(),
            kind,
            account: SourceAccountRef::new(MessageSource::NapCat, "account").unwrap(),
            thread_ids: thread_ids
                .iter()
                .map(|id| EventThreadId::new(*id).unwrap())
                .collect(),
            affected_event_count: 1,
            affected_conversation_count: 2,
            affected_source_event_ids: vec![SourceEventId::new("event").unwrap()],
            reason: "Owner 请求纠正线程归属".into(),
        }
    }

    #[test]
    fn merge_preview_suspends_without_effects() {
        let result = suspend_thread_mutation_for_approval(impact(
            ThreadMutationKind::Merge,
            &["thread-a", "thread-b"],
        ))
        .unwrap();
        match result {
            NodeResult::Suspend {
                request, effects, ..
            } => {
                assert_eq!(request.reason, SuspendReason::Approval);
                assert_eq!(request.data.kind, ThreadMutationKind::Merge);
                assert!(effects.is_empty());
            }
            NodeResult::Continue { .. } => panic!("high-impact mutation must suspend"),
        }
    }

    #[test]
    fn invalid_merge_never_reaches_suspend() {
        let error =
            suspend_thread_mutation_for_approval(impact(ThreadMutationKind::Merge, &["thread-a"]))
                .unwrap_err();
        assert!(matches!(error, ThreadMutationError::InvalidImpact(_)));
    }

    #[test]
    fn revert_requires_a_bounded_reason() {
        let input = ThreadMutationRevertInput {
            proposal_id: ThreadMutationProposalId::new("proposal").unwrap(),
            command_source_event_id: SourceEventId::new("command").unwrap(),
            reason: " ".into(),
        };
        assert!(matches!(
            validate_thread_mutation_revert(&input),
            Err(ThreadMutationError::InvalidRevert(_))
        ));
    }
}
