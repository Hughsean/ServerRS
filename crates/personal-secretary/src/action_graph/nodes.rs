//! Action Graph 节点：Plan / L0Execute / BuildResponse / NoAction。
//!
//! Graph 拓扑（约束 4：Effect 只能通过 EffectExecutor 执行一次）：
//! `Plan -> (Gate 内联) -> L0Execute -> BuildResponse -> End`
//! 以及 `Plan -> Suspend -> End`（挂起后由 Checkpoint 恢复）。

use std::sync::Arc;

use agent_core::graph::{
    AgentNode, NodeError, NodeErrorKind, NodeId, NodeResult, RunContext, UsageDelta,
};
use agent_core::{AgentOutcome, AgentState, AgentUpdate};
use async_trait::async_trait;

use crate::{
    ConversationKind, ConversationRef, EventQuery, OwnerResponseDraft, PlannerInput, PlannerOutput,
    PlannerRetrievedExcerpt, ResponseSegment, SecretaryAction, SecretaryActionApprovalRequest,
    SecretaryActionEffect, SecretaryActionProposal, SecretaryAgentState, SecretaryAgentUpdate,
    gate_secretary_action, validate_planner_output,
};

use super::port::ActionRunContext;

#[derive(Debug, thiserror::Error)]
#[error("action graph error: {0}")]
pub struct ActionGraphError(pub(crate) String);

impl ActionGraphError {
    pub(crate) fn from_display<E: std::fmt::Display>(e: E) -> Self {
        Self(e.to_string())
    }
}

/// Plan 节点：先检索相关事件，再调用 Planner 生成 Proposal/NoAction/Clarification。
/// P0-2 修复：接入 RetrieverUseCase，让 L0 Action 有真实数据库证据输入。
pub struct PlanNode {
    id: NodeId,
    planner: Arc<dyn crate::ActionPlannerT>,
    retriever: Option<Arc<crate::RetrieverUseCase>>,
    context: Arc<ActionRunContext>,
}

impl PlanNode {
    pub fn new(
        planner: Arc<dyn crate::ActionPlannerT>,
        retriever: Option<Arc<crate::RetrieverUseCase>>,
        context: Arc<ActionRunContext>,
    ) -> Result<Self, ActionGraphError> {
        Ok(Self {
            id: NodeId::try_from("plan").map_err(ActionGraphError::from_display)?,
            planner,
            retriever,
            context,
        })
    }
}

#[async_trait]
impl AgentNode<SecretaryAgentState> for PlanNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<SecretaryAgentState>,
        _context: &RunContext,
    ) -> Result<
        NodeResult<SecretaryAgentUpdate, SecretaryActionEffect, SecretaryActionApprovalRequest>,
        NodeError,
    > {
        let business = state.business();
        // P0-2 修复：检索相关事件作为 Planner 的证据输入。
        // 只检索允许进入模型的 Normal 内容；local_only 等由 RetrieverPolicy 过滤。
        let retrieved = if let Some(retriever) = &self.retriever {
            let query = EventQuery {
                account: self.context.account.clone(),
                conversation: Some(
                    ConversationRef::new(
                        ConversationKind::OwnerControl,
                        &self.context.conversation_id,
                    )
                    .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?,
                ),
                actor_id: None,
                thread_id: None,
                since_unix_secs: Some(self.context.occurred_at_unix_secs - 86_400),
                until_unix_secs: Some(self.context.occurred_at_unix_secs),
                query_text: Some(self.context.command_text.clone()),
                limit: 20,
            };
            retriever
                .search_events(&query, false)
                .await
                .map_err(|e| NodeError::with_source(NodeErrorKind::Transient, e))?
                .into_iter()
                .map(|r| PlannerRetrievedExcerpt {
                    source_event_id: r.source_event_id,
                    excerpt: r.excerpt,
                    occurred_at_unix_secs: r.occurred_at_unix_secs,
                    actor_id: r.actor.id,
                })
                .collect()
        } else {
            Vec::new()
        };
        let input = PlannerInput {
            account: self.context.account.clone(),
            command: crate::PlannerCommandEvent {
                source_event_id: self.context.command_source_event_id.clone(),
                conversation: ConversationRef::new(
                    ConversationKind::OwnerControl,
                    &self.context.conversation_id,
                )
                .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?,
                occurred_at_unix_secs: self.context.occurred_at_unix_secs,
                normalized_text: self.context.command_text.clone(),
            },
            recent_events: business.recent_events().to_vec(),
            timezone_offset_secs: self.context.timezone_offset_secs,
            now_unix_secs: self.context.now_unix_secs,
            retrieved,
        };
        crate::validate_planner_input(&input)
            .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?;
        let output = self
            .planner
            .plan(&input)
            .await
            .map_err(|e| NodeError::with_source(NodeErrorKind::Transient, e))?;
        validate_planner_output(&output)
            .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?;
        match output {
            PlannerOutput::NoAction { reason } => Ok(NodeResult::new(
                vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(reason))],
                UsageDelta::default(),
            )),
            PlannerOutput::Clarification { question, evidence } => {
                let proposal = SecretaryActionProposal::new(
                    SecretaryAction::AskOwnerClarification { question },
                    "Planner 请求 Owner 澄清",
                    evidence,
                    None,
                )
                .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?;
                gate_secretary_action(proposal)
                    .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))
            }
            PlannerOutput::Proposal(proposal) => gate_secretary_action(proposal)
                .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e)),
        }
    }
}

/// L0Execute 节点：只读 Action 的 Effect 由 PlanNode 通过 gate 已返回，
/// 此节点用于 Suspend 恢复后的流转占位。
pub struct L0ExecuteNode {
    id: NodeId,
}

impl L0ExecuteNode {
    pub fn new() -> Result<Self, ActionGraphError> {
        Ok(Self {
            id: NodeId::try_from("l0_execute").map_err(ActionGraphError::from_display)?,
        })
    }
}

#[async_trait]
impl AgentNode<SecretaryAgentState> for L0ExecuteNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        _state: &AgentState<SecretaryAgentState>,
        _context: &RunContext,
    ) -> Result<
        NodeResult<SecretaryAgentUpdate, SecretaryActionEffect, SecretaryActionApprovalRequest>,
        NodeError,
    > {
        Ok(NodeResult::empty())
    }
}

// ===== BuildResponse 节点（P0 修复：Graph 必须产生 AgentOutcome）=====

/// BuildResponse 节点：从 last_receipt 构造 OwnerResponseDraft，设置 ResponseReady + Outcome。
/// 在 L0Execute 后执行，确保 Effect 执行完毕后 Graph 能正常终止。
pub struct BuildResponseNode {
    id: NodeId,
    context: Arc<ActionRunContext>,
}

impl BuildResponseNode {
    pub fn new(context: Arc<ActionRunContext>) -> Result<Self, ActionGraphError> {
        Ok(Self {
            id: NodeId::try_from("build_response").map_err(ActionGraphError::from_display)?,
            context,
        })
    }
}

#[async_trait]
impl AgentNode<SecretaryAgentState> for BuildResponseNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<SecretaryAgentState>,
        _context: &RunContext,
    ) -> Result<
        NodeResult<SecretaryAgentUpdate, SecretaryActionEffect, SecretaryActionApprovalRequest>,
        NodeError,
    > {
        let business = state.business();
        // NoAction 路径：Plan 已设置 Outcome，无需重复构建
        if state.outcome().is_some() {
            return Ok(NodeResult::empty());
        }
        // Effect 路径：从 last_receipt 的真实 result_ref 构建摘要
        let mut source_ids: Vec<crate::SourceEventId> = Vec::new();
        for id in business.evidence_source_event_ids() {
            if !source_ids.contains(id) {
                source_ids.push(id.clone());
            }
        }
        let summary = if let Some(receipt) = business.last_receipt() {
            format!("动作已执行：{}", receipt.result_ref)
        } else {
            "已处理".into()
        };
        let segments = vec![ResponseSegment::Summary { text: summary }];
        let draft = OwnerResponseDraft::new(segments, source_ids, self.context.now_unix_secs)
            .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?;
        Ok(NodeResult::new(
            vec![
                AgentUpdate::Business(SecretaryAgentUpdate::ResponseReady(draft.clone())),
                AgentUpdate::SetOutcome(AgentOutcome::Respond(
                    draft
                        .segments()
                        .first()
                        .map(|s| s.text().to_owned())
                        .unwrap_or_default(),
                )),
            ],
            UsageDelta::default(),
        ))
    }
}

/// NoAction 终止节点。
pub struct NoActionNode {
    id: NodeId,
}

impl NoActionNode {
    pub fn new() -> Result<Self, ActionGraphError> {
        Ok(Self {
            id: NodeId::try_from("no_action").map_err(ActionGraphError::from_display)?,
        })
    }
}

#[async_trait]
impl AgentNode<SecretaryAgentState> for NoActionNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        _state: &AgentState<SecretaryAgentState>,
        _context: &RunContext,
    ) -> Result<
        NodeResult<SecretaryAgentUpdate, SecretaryActionEffect, SecretaryActionApprovalRequest>,
        NodeError,
    > {
        Ok(NodeResult::empty())
    }
}
