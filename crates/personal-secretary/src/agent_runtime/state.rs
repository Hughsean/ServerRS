//! Agent 工作状态、Phase、Update 与 `AgentBusinessState` 实现。
//!
//! 有界工作状态：原始消息正文和完整工具结果始终留在外部事件日志中。
//! 本模块只持有指针与有界摘要。

use agent_core::{AgentBusinessState, AgentStateError, AgentUpdate};
use serde::{Deserialize, Serialize};

use crate::SourceEventId;
use crate::planner::PlannerToolObservation;

use super::action::{SecretaryActionEffect, SecretaryActionProposal, SecretaryActionReceipt};
use super::approval::SecretaryApprovalDecision;
use super::response::{OwnerResponseDraft, RecentEventRef};
use super::validation::{
    SecretaryAgentRuntimeError, validate_action_proposal, validate_agent_state,
    validate_response_draft,
};
use super::working_context::{
    AgentWorkingContextV1, WorkingContextError, WorkingContextProjection, WorkingContextUpdate,
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
    /// ReplanDecision 节点追加一条工具观察到状态。
    ObservationAppended(PlannerToolObservation),
    /// 跨阶段工作上下文的类型化更新（CMD-009 目标 A）。节点不得绕过状态机
    /// 偷偷修改状态，只能通过此更新进入。
    WorkingContext(WorkingContextUpdate),
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
    /// Replan 轮次（0-based）。首次 Plan 时为 0。
    #[serde(default)]
    replan_round: u8,
    /// Replan 过程中收集的工具观察。供下一轮 Planner 输入。
    #[serde(default)]
    planning_observations: Vec<PlannerToolObservation>,
    /// 跨阶段有界工作上下文（CMD-009 目标 A）。只保存结构化引用与未决状态，
    /// 不保存完整消息正文；旧 Checkpoint 缺少该字段时安全恢复为 None。
    #[serde(default)]
    working_context: Option<AgentWorkingContextV1>,
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
            replan_round: 0,
            planning_observations: Vec::new(),
            working_context: None,
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

    /// Replan 轮次（0-based）。首次 Plan 时为 0。
    pub fn replan_round(&self) -> u8 {
        self.replan_round
    }

    /// Replan 过程中收集的工具观察。
    pub fn planning_observations(&self) -> &[PlannerToolObservation] {
        &self.planning_observations
    }

    /// 跨阶段有界工作上下文（旧 Checkpoint 为 None）。
    pub fn working_context(&self) -> Option<&AgentWorkingContextV1> {
        self.working_context.as_ref()
    }

    /// Planner 接收的有界工作上下文投影（内部真实 ID；LLM 适配层映射为临时引用）。
    pub fn working_context_projection(&self) -> Option<WorkingContextProjection> {
        self.working_context
            .as_ref()
            .map(AgentWorkingContextV1::projection)
    }

    /// 合并类型化工作上下文更新。合并后重新校验硬上限，超限 fail-closed。
    fn apply_working_context_update(
        &mut self,
        update: WorkingContextUpdate,
    ) -> Result<(), WorkingContextError> {
        // 在副本上合并并校验，避免超限或非法更新返回错误后留下半更新状态。
        let mut context = self.working_context.clone().unwrap_or_default();
        match update {
            WorkingContextUpdate::InitialRetrieval {
                evidence_refs,
                resolved_conversation_refs,
                trigger,
            } => {
                context.merge_initial_retrieval(
                    evidence_refs,
                    resolved_conversation_refs,
                    trigger,
                )?;
            }
            WorkingContextUpdate::ReplanEvidence {
                evidence_refs,
                resolved_thread_refs,
                resolved_participant_refs,
                resolved_fact_refs,
                open_references,
            } => {
                context.merge_replan_evidence(
                    evidence_refs,
                    resolved_thread_refs,
                    resolved_participant_refs,
                    resolved_fact_refs,
                    open_references,
                )?;
            }
            WorkingContextUpdate::ConflictReRead(conflict) => {
                context.merge_conflict(conflict)?;
            }
        }
        self.working_context = Some(context);
        Ok(())
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
            SecretaryAgentUpdate::ActionCompleted(mut receipt) => {
                let pending = self.pending_proposal.as_ref().ok_or_else(|| {
                    AgentStateError::Business("动作回执缺少待执行 Proposal".into())
                })?;
                if pending.proposal_id != receipt.proposal_id {
                    return Err(AgentStateError::Business(
                        "动作回执与待执行 Proposal 不匹配".into(),
                    ));
                }
                // 以 pending proposal 的 action.kind() 为规范值，强制 tool_kind 一致性：
                // - 显式不一致 → 拒绝
                // - 缺失（历史回执/超时等）→ 补齐
                let canonical = pending.action.kind();
                match &receipt.tool_kind {
                    Some(kind) if *kind != canonical => {
                        return Err(AgentStateError::Business(format!(
                            "回执 tool_kind {:?} 与 Proposal {:?} 不一致",
                            kind, canonical,
                        )));
                    }
                    None => {
                        receipt.tool_kind = Some(canonical);
                    }
                    _ => {} // 一致，无需修改
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
            SecretaryAgentUpdate::ObservationAppended(obs) => {
                crate::planner::validate_tool_observation(&obs)
                    .map_err(|error| AgentStateError::Business(error.to_string()))?;
                // 同 proposal 不重复追加
                if self
                    .planning_observations
                    .iter()
                    .any(|existing| existing.proposal_id == obs.proposal_id)
                {
                    return Ok(());
                }
                self.planning_observations.push(obs);
                self.replan_round = self.replan_round.saturating_add(1);
                self.phase = SecretaryAgentPhase::UpdateState;
            }
            // CMD-009 目标 A：工作上下文只通过类型化更新进入状态机；合并后重新
            // 校验硬上限，超限 fail-closed（不做静默截断）。
            SecretaryAgentUpdate::WorkingContext(update) => {
                self.apply_working_context_update(update)
                    .map_err(|error| AgentStateError::Business(error.to_string()))?;
            }
            SecretaryAgentUpdate::PhaseChanged(phase) => self.phase = phase,
        }
        Ok(())
    }
}
