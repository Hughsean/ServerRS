//! Action Graph 节点：Plan / L0Execute / BuildResponse / NoAction。
//!
//! Graph 拓扑（约束 4：Effect 只能通过 EffectExecutor 执行一次）：
//! `Plan -> (Gate 内联) -> L0Execute -> BuildResponse -> End`
//! 以及 `Plan -> Suspend -> End`（挂起后由 Checkpoint 恢复）。

use std::sync::Arc;

use agent_core::graph::{
    AgentNode, NodeError, NodeErrorKind, NodeId, NodeResult, RouteKey, Router, RunContext,
    UsageDelta,
};
use agent_core::{AgentOutcome, AgentState, AgentUpdate};
use async_trait::async_trait;

use crate::planner::MAX_RECENT_EVENT_VIEWS;
use crate::{
    ConversationKind, ConversationRef, EventQuery, PlannerInput, PlannerOutput,
    PlannerRetrievedExcerpt, SecretaryAction, SecretaryActionApprovalRequest,
    SecretaryActionEffect, SecretaryActionProposal, SecretaryAgentPhase, SecretaryAgentState,
    SecretaryAgentUpdate, gate_secretary_action, is_replan_observation_tool,
    validate_planner_output,
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
        // CTX-003：从事件仓储填充最近事件窗口（发送者、@、Reply、Thread、内容策略）。
        let recent_event_views = if let Some(retriever) = &self.retriever {
            retriever
                .list_recent_event_views(
                    &self.context.account,
                    MAX_RECENT_EVENT_VIEWS as u16,
                    self.context.is_local_loopback,
                )
                .await
                .map_err(|e| NodeError::with_source(NodeErrorKind::Transient, e))?
        } else {
            Vec::new()
        };
        let replan_round = business.replan_round();
        let budget_spent = replan_round.min(crate::planner::MAX_REPLAN_ROUNDS);
        let remaining_query_budget = crate::planner::MAX_REPLAN_ROUNDS.saturating_sub(budget_spent);
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
            recent_event_views,
            timezone_offset_secs: self.context.timezone_offset_secs,
            timezone: self.context.timezone.clone(),
            now_unix_secs: self.context.now_unix_secs,
            retrieved,
            observations: business.planning_observations().to_vec(),
            replan_round,
            remaining_query_budget,
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

/// L0Execute 节点：L0/L1 Action 的 Effect 已在 Plan 节点通过 gate 返回；
/// 此节点也为 Suspend→Approve 恢复后的 L2 Action 生成 Effect，使其能通过
/// EffectExecutor 完成策略持久化。
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
        state: &AgentState<SecretaryAgentState>,
        _context: &RunContext,
    ) -> Result<
        NodeResult<SecretaryAgentUpdate, SecretaryActionEffect, SecretaryActionApprovalRequest>,
        NodeError,
    > {
        let business = state.business();
        // Suspend→Approve 恢复后 phase 为 Execute 且 pending_proposal 未清除；
        // 此时必须生成 Effect 以使 EffectExecutor 真正执行策略持久化。
        // L0/L1 Action 的 pending_proposal 已在 Plan→Effect→ActionCompleted 中被清除，
        // 因此该分支不会重复执行它们。
        if business.phase() == SecretaryAgentPhase::Execute
            && let Some(proposal) = business.pending_proposal()
        {
            return Ok(NodeResult::with_effect(
                Vec::new(),
                SecretaryActionEffect {
                    proposal: proposal.clone(),
                },
                UsageDelta::default(),
            ));
        }
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
        // Replan 第二轮 NoAction / 最终回答路径：Plan 已设置 Outcome。
        // 从 Outcome 文本构造 ResponseReady，不依赖 last_receipt。
        if let Some(outcome) = state.outcome() {
            if let Some(text) = outcome.response_text() {
                let draft = crate::OwnerResponseDraft::new(
                    vec![crate::ResponseSegment::Summary {
                        text: text.to_string(),
                    }],
                    business.evidence_source_event_ids().to_vec(),
                    self.context.now_unix_secs,
                )
                .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?;
                return Ok(NodeResult::new(
                    vec![AgentUpdate::Business(SecretaryAgentUpdate::ResponseReady(
                        draft,
                    ))],
                    UsageDelta::default(),
                ));
            }
            return Ok(NodeResult::empty());
        }
        // Effect 路径 / Replan 预算耗尽路径：从 last_receipt 构造响应。
        // 若 result_ref 可解析为 QueryEffectResultV1，使用其人类可读 summary；
        // 避免将结构化 JSON 作为 Owner 可见文案。
        let mut source_ids: Vec<crate::SourceEventId> = Vec::new();
        for id in business.evidence_source_event_ids() {
            if !source_ids.contains(id) {
                source_ids.push(id.clone());
            }
        }
        let draft = if let Some(receipt) = business.last_receipt() {
            if let Ok(q) = serde_json::from_str::<crate::QueryEffectResultV1>(&receipt.result_ref) {
                let bounded: String = q.summary.chars().take(500).collect();
                crate::OwnerResponseDraft::new(
                    vec![crate::ResponseSegment::Summary {
                        text: format!("查询完成：{bounded}"),
                    }],
                    source_ids,
                    self.context.now_unix_secs,
                )
                .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?
            } else {
                crate::build_action_response_draft(
                    Some(receipt),
                    source_ids,
                    self.context.now_unix_secs,
                )
                .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?
            }
        } else {
            crate::build_action_response_draft(None, source_ids, self.context.now_unix_secs)
                .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?
        };
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

// ===== ReplanDecisionNode =====

/// Replan 决策节点：从 last_receipt 提取查询观察并决定是否继续循环。
///
/// 此节点将 EffectExecutor 产生的结构化 JSON result_ref 解析为
/// `PlannerToolObservation`，并追加到状态中。Replan 循环是否继续
/// 由 `ReplanRouter` 在状态更新后独立判断。
pub struct ReplanDecisionNode {
    id: NodeId,
}

impl ReplanDecisionNode {
    pub fn new() -> Result<Self, ActionGraphError> {
        Ok(Self {
            id: NodeId::try_from("replan_decision").map_err(ActionGraphError::from_display)?,
        })
    }
}

#[async_trait]
impl AgentNode<SecretaryAgentState> for ReplanDecisionNode {
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

        // 已有 Outcome → 不追加观察，由 Router 判定 finish。
        if state.outcome().is_some() {
            return Ok(NodeResult::empty());
        }

        let Some(receipt) = business.last_receipt() else {
            return Ok(NodeResult::empty());
        };

        let Some(tool_kind) = receipt.tool_kind else {
            return Ok(NodeResult::empty());
        };

        // 只有允许 Replan 的只读查询工具才尝试解析观察。
        if !is_replan_observation_tool(tool_kind) {
            return Ok(NodeResult::empty());
        }

        // 尝试把 result_ref 解析为 QueryEffectResultV1。
        // 解析失败（旧格式或非结构化回执）→ 保守终止 Replan。
        let query_result: crate::QueryEffectResultV1 =
            match serde_json::from_str(&receipt.result_ref) {
                Ok(v) => v,
                Err(_) => {
                    tracing::debug!(
                        proposal_id = %receipt.proposal_id,
                        "result_ref 无法解析为 QueryEffectResultV1，跳过 Replan"
                    );
                    return Ok(NodeResult::empty());
                }
            };

        // 结构化回执一致性校验：版本必须为 1，工具类型必须与 receipt 一致。
        if query_result.version != 1 {
            tracing::warn!(
                proposal_id = %receipt.proposal_id,
                version = query_result.version,
                "QueryEffectResultV1 版本不匹配，跳过 Replan"
            );
            return Ok(NodeResult::empty());
        }
        if query_result.tool_kind != tool_kind {
            tracing::warn!(
                proposal_id = %receipt.proposal_id,
                result_tool = ?query_result.tool_kind,
                receipt_tool = ?tool_kind,
                "QueryEffectResultV1.tool_kind 与 receipt.tool_kind 不一致，跳过 Replan"
            );
            return Ok(NodeResult::empty());
        }

        // 同 proposal 去重已在 apply_update 中处理。
        let observation = query_result.to_observation(receipt.proposal_id.clone(), true);

        Ok(NodeResult::new(
            vec![AgentUpdate::Business(
                SecretaryAgentUpdate::ObservationAppended(observation),
            )],
            UsageDelta::default(),
        ))
    }
}

// ===== ReplanRouter =====

/// Replan 路由选择器：基于状态中的 replan_round、last_receipt 和 Outcome
/// 决定是继续 Plan（continue）还是进入 BuildResponse（finish）。
pub struct ReplanRouter;

impl Router<SecretaryAgentState> for ReplanRouter {
    fn known_routes(&self) -> Vec<RouteKey> {
        vec![
            RouteKey::try_from("continue").unwrap(),
            RouteKey::try_from("finish").unwrap(),
        ]
    }

    fn select(&self, state: &AgentState<SecretaryAgentState>) -> Result<RouteKey, NodeError> {
        let business = state.business();

        // 已有 Outcome → 直接结束。
        if state.outcome().is_some() {
            return Ok(RouteKey::try_from("finish").unwrap());
        }

        // 没有 receipt → 结束。
        let Some(receipt) = business.last_receipt() else {
            return Ok(RouteKey::try_from("finish").unwrap());
        };

        // 最新工具不是允许 Replan 的查询工具 → 结束。
        let Some(tool_kind) = receipt.tool_kind else {
            return Ok(RouteKey::try_from("finish").unwrap());
        };
        if !is_replan_observation_tool(tool_kind) {
            return Ok(RouteKey::try_from("finish").unwrap());
        }

        // 预算耗尽 → 结束。
        if business.replan_round() >= crate::planner::MAX_REPLAN_ROUNDS {
            return Ok(RouteKey::try_from("finish").unwrap());
        }

        // 已追加观察 → 继续 Plan。
        if business
            .planning_observations()
            .iter()
            .any(|o| o.proposal_id == receipt.proposal_id)
        {
            return Ok(RouteKey::try_from("continue").unwrap());
        }

        // 观察未成功解析（result_ref 不可解析）→ 结束。
        Ok(RouteKey::try_from("finish").unwrap())
    }
}
