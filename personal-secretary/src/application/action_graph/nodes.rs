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
    PlannerRetrievedExcerpt, RetrievalTriggerKind, SecretaryAction, SecretaryActionApprovalRequest,
    SecretaryActionEffect, SecretaryActionProposal, SecretaryAgentPhase, SecretaryAgentState,
    SecretaryAgentUpdate, SecretaryToolKind, WorkingContextUpdate, gate_secretary_action,
    is_replan_observation_tool, validate_planner_output,
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
        // CMD-009 目标 B：初始检索默认按账号范围检索，不再强制限制为 OwnerControl
        // 会话；未指定 since 时可检索 24 小时以前的长期事件（不暗中补 24h 下限）。
        // “相关历史检索”与“最近事件窗口”（recent_event_views，固定 ≤8 条）分离。
        let retrieved = if let Some(retriever) = &self.retriever {
            let query = EventQuery {
                account: self.context.account.clone(),
                conversation: None,
                actor_id: None,
                thread_id: None,
                since_unix_secs: None,
                until_unix_secs: None,
                query_text: Some(self.context.command_text.clone()),
                limit: 20,
            };
            retriever
                .search_events(&query, self.context.is_local_loopback)
                .await
                .map_err(|e| NodeError::with_source(NodeErrorKind::Transient, e))?
                .into_iter()
                .map(|r| PlannerRetrievedExcerpt {
                    source_event_id: r.source_event_id,
                    excerpt: r.excerpt,
                    occurred_at_unix_secs: r.occurred_at_unix_secs,
                    actor_id: r.actor.id,
                    actor_kind: r.actor.kind,
                })
                .collect()
        } else {
            Vec::new()
        };
        // 登记本轮已选择证据引用（去重有界由工作上下文校验保证）。
        let evidence_refs: Vec<crate::SourceEventId> = retrieved
            .iter()
            .map(|r| r.source_event_id.clone())
            .collect();
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
        // CMD-009 目标 A：Planner 只接收有界工作上下文投影；状态只保存引用，
        // 每轮重新读取正文与内容策略，撤回/envelope_only/never_long_term 后
        // 旧正文不可继续使用（投影本身不含正文）。
        let working_context = business.working_context_projection();
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
            working_context,
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
        // CMD-010 防线 B：证据门属于领域边界，不能只依赖某个 LLM 适配器。
        // 任何 Planner 实现产生的非 L0 Proposal 都必须引用本轮权威 OwnerCommand。
        if let PlannerOutput::Proposal(ref proposal) = output
            && proposal.action.kind().policy().risk > crate::SecretaryRiskLevel::L0ReadOnly
            && !proposal
                .source_event_ids
                .contains(&self.context.command_source_event_id)
        {
            return Err(NodeError::with_source(
                NodeErrorKind::Invariant,
                crate::PlannerError::InvalidOutput(format!(
                    "{:?} 写动作缺少本轮 OwnerCommand 证据",
                    proposal.action.kind()
                )),
            ));
        }
        // CMD-010 防线 C：上一轮产生未解决指代时，只允许向 Owner 澄清；
        // 不允许模型利用歧义候选直接构造任何写动作或继续猜测其他 Action。
        if let PlannerOutput::Proposal(ref proposal) = output
            && business
                .working_context()
                .is_some_and(|working| !working.open_references.is_empty())
            && !matches!(
                proposal.action,
                crate::SecretaryAction::AskOwnerClarification { .. }
            )
        {
            return Err(NodeError::with_source(
                NodeErrorKind::Invariant,
                crate::PlannerError::DisallowedAction("存在未解决指代时只能请求 Owner 澄清".into()),
            ));
        }
        // CMD-009 目标 C：冲突轮次（工作上下文已有未解决冲突）只允许向 Owner 解释、
        // 请求澄清或提议仍需 L2 审批的修正动作；绝不能自动再次执行原
        // ApproveMemoryCandidate。结构上强制，不依赖模型自律。
        if let PlannerOutput::Proposal(ref proposal) = output
            && business
                .working_context()
                .and_then(|w| w.conflict.as_ref())
                .is_some()
            && !crate::is_allowed_after_memory_conflict(&proposal.action)
        {
            return Err(NodeError::with_source(
                NodeErrorKind::Invariant,
                crate::PlannerError::DisallowedAction(format!(
                    "{:?} 在记忆候选冲突轮次不允许执行",
                    proposal.action.kind()
                )),
            ));
        }
        // 本轮初始/Replan 检索进入工作上下文（状态更新必须经过类型化
        // SecretaryAgentUpdate，节点不得绕过状态机）。
        let command_conversation = ConversationRef::new(
            ConversationKind::OwnerControl,
            &self.context.conversation_id,
        )
        .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?;
        let working_update = AgentUpdate::Business(SecretaryAgentUpdate::WorkingContext(
            WorkingContextUpdate::InitialRetrieval {
                evidence_refs,
                resolved_conversation_refs: vec![command_conversation],
                trigger: RetrievalTriggerKind::InitialOwnerCommand,
            },
        ));
        match output {
            PlannerOutput::NoAction { reason } => Ok(NodeResult::new(
                vec![
                    working_update,
                    AgentUpdate::SetOutcome(AgentOutcome::Respond(reason)),
                ],
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
                Ok(prepend_update(
                    gate_secretary_action(proposal)
                        .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?,
                    working_update,
                ))
            }
            PlannerOutput::Proposal(proposal) => Ok(prepend_update(
                gate_secretary_action(proposal)
                    .map_err(|e| NodeError::with_source(NodeErrorKind::Invariant, e))?,
                working_update,
            )),
        }
    }
}

/// 把一条更新前置到已有 NodeResult 的 updates 中（保持 NodeResult 类型不变）。
fn prepend_update(
    result: NodeResult<SecretaryAgentUpdate, SecretaryActionEffect, SecretaryActionApprovalRequest>,
    update: AgentUpdate<SecretaryAgentUpdate>,
) -> NodeResult<SecretaryAgentUpdate, SecretaryActionEffect, SecretaryActionApprovalRequest> {
    match result {
        NodeResult::Continue {
            updates,
            effects,
            usage,
        } => NodeResult::Continue {
            updates: std::iter::once(update).chain(updates).collect(),
            effects,
            usage,
        },
        NodeResult::Suspend {
            updates,
            effects,
            usage,
            request,
        } => NodeResult::Suspend {
            updates: std::iter::once(update).chain(updates).collect(),
            effects,
            usage,
            request,
        },
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
                for id in q.source_event_ids {
                    if !source_ids.contains(&id) {
                        source_ids.push(id);
                    }
                }
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
///
/// CMD-009 目标 C：当回执是记忆候选批准冲突（`MemoryCandidateConflictResultV1`）
/// 时，本节点通过 `MemoryUseCase::evidence` 执行一次 L0 回读（现行事实与有效来源，
/// 重新检查账号、事实状态、撤回与内容策略），回读结果进入工作上下文并允许
/// 恰好一次 Replan；绝不重复写批准审计（回读是 L0 查询）。
pub struct ReplanDecisionNode {
    id: NodeId,
    /// 冲突回读用的 MemoryUseCase（未注入时冲突回读 fail-closed 为说明文案）。
    memory: Option<Arc<crate::MemoryUseCase>>,
    /// 回读账号作用域（冲突事实必须属于本账号）。
    account: Option<crate::SourceAccountRef>,
    /// 仅经过配置验证的本地模型路径可读取 local_only 事实来源。
    is_local_loopback: bool,
}

impl ReplanDecisionNode {
    pub fn new() -> Result<Self, ActionGraphError> {
        Ok(Self {
            id: NodeId::try_from("replan_decision").map_err(ActionGraphError::from_display)?,
            memory: None,
            account: None,
            is_local_loopback: false,
        })
    }

    /// 注入冲突回读依赖（CMD-009 目标 C）。
    pub fn with_conflict_re_read(
        mut self,
        memory: Arc<crate::MemoryUseCase>,
        account: crate::SourceAccountRef,
        is_local_loopback: bool,
    ) -> Self {
        self.memory = Some(memory);
        self.account = Some(account);
        self.is_local_loopback = is_local_loopback;
        self
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

        // CMD-009 目标 C：记忆候选批准冲突回执 → 执行一次 L0 回读（现行事实与
        // 有效来源），结果进入工作上下文并允许恰好一次 Replan。冲突是确定性
        // 业务结果：不自动覆盖、不 supersede、不重放原批准；回读不写批准审计。
        if tool_kind == SecretaryToolKind::ApproveMemoryCandidate
            && let Ok(conflict_result) =
                serde_json::from_str::<crate::MemoryCandidateConflictResultV1>(&receipt.result_ref)
            && conflict_result.version == 1
        {
            // 同一候选冲突已回读（Checkpoint 恢复 / 幂等重放）→ 幂等跳过。
            let already_recorded = business
                .working_context()
                .and_then(|w| w.conflict.as_ref())
                .is_some_and(|c| c.candidate_id == conflict_result.candidate_id);
            if already_recorded {
                return Ok(NodeResult::empty());
            }
            let conflict_context = self.re_read_conflict(&conflict_result).await;
            return Ok(NodeResult::new(
                vec![AgentUpdate::Business(SecretaryAgentUpdate::WorkingContext(
                    WorkingContextUpdate::ConflictReRead(conflict_context),
                ))],
                UsageDelta::default(),
            ));
        }

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

        // CMD-009 目标 A：观察进入工作上下文——登记新证据引用、从 typed_events
        // 派生已解析参与者引用，歧义观察登记为未解决指代（保序去重由状态机保证）。
        let resolved_participant_refs: Vec<crate::ParticipantRef> = observation
            .typed_events
            .iter()
            .map(|te| crate::ParticipantRef {
                platform_kind: te.actor_kind,
                stable_id: te.actor_id.clone(),
            })
            .collect();
        let open_references = if observation.ambiguous {
            vec![crate::OpenReference {
                kind: crate::OpenReferenceKind::AmbiguousReference,
                // 工具 summary 可能含平台稳定 ID；工作上下文只保存固定类型化文案。
                label: "存在未解决的指代".into(),
                source_event_ids: observation.source_event_ids.clone(),
                reason: "指代解析存在多个候选，需要 Owner 澄清".into(),
            }]
        } else {
            Vec::new()
        };
        let working_update = AgentUpdate::Business(SecretaryAgentUpdate::WorkingContext(
            WorkingContextUpdate::ReplanEvidence {
                evidence_refs: observation.source_event_ids.clone(),
                resolved_thread_refs: Vec::new(),
                resolved_participant_refs,
                resolved_fact_refs: Vec::new(),
                open_references,
            },
        ));

        Ok(NodeResult::new(
            vec![
                AgentUpdate::Business(SecretaryAgentUpdate::ObservationAppended(observation)),
                working_update,
            ],
            UsageDelta::default(),
        ))
    }
}

impl ReplanDecisionNode {
    /// 冲突回读：通过 `MemoryUseCase::evidence` 重新读取现行事实与有效来源。
    ///
    /// 重新检查：账号作用域（事实必须属于本账号）、事实状态（proposed/confirmed）、
    /// 撤回与内容策略（来源集合必须完整覆盖事实引用的全部来源，任一关键来源
    /// 失效即 fail-closed，不把旧事实呈现为有效）。
    async fn re_read_conflict(
        &self,
        conflict_result: &crate::MemoryCandidateConflictResultV1,
    ) -> crate::MemoryCandidateConflictContext {
        use crate::{MemoryCandidateConflictContext, MemoryConflictReasonCode};
        let candidate_id = conflict_result.candidate_id.clone();
        let fact_id = conflict_result.fact_id.clone();
        let fallback = |reason_code: MemoryConflictReasonCode, summary: &str| {
            MemoryCandidateConflictContext::invalid(
                candidate_id.clone(),
                fact_id.clone(),
                reason_code,
                summary,
            )
            .unwrap_or_else(|_| {
                // 理论上不可达：兜底构造仍失败时用最短安全文案。
                MemoryCandidateConflictContext::invalid(
                    candidate_id,
                    fact_id,
                    MemoryConflictReasonCode::ReReadFailed,
                    "记忆候选与现行记忆存在冲突，来源信息暂不可用，请人工复核。",
                )
                .expect("minimal conflict context must validate")
            })
        };
        let Some(memory) = &self.memory else {
            return fallback(
                MemoryConflictReasonCode::ReReadFailed,
                "记忆候选与现行记忆存在冲突，但回读能力未配置，请人工复核。",
            );
        };
        match memory.evidence(&conflict_result.fact_id, 800).await {
            Ok(Some(view)) => {
                let Some(account) = self.account.as_ref() else {
                    return fallback(
                        MemoryConflictReasonCode::ReReadAccountMismatch,
                        "记忆候选与现行记忆存在冲突，但当前账号作用域缺失，请人工复核。",
                    );
                };
                if &view.fact.account != account {
                    return fallback(
                        MemoryConflictReasonCode::ReReadAccountMismatch,
                        "记忆候选与现行记忆存在冲突，但现行记忆不属于当前账号，请人工复核。",
                    );
                }
                // 未经 Owner 确认的 Proposed 事实不能作为现行长期事实重新入模。
                if view.fact.status != crate::MemoryFactStatus::Confirmed {
                    return fallback(
                        MemoryConflictReasonCode::ReReadSourcesInvalidated,
                        "记忆候选与现行记忆存在冲突，但现行记忆已失效，请人工复核。",
                    );
                }
                // 撤回/内容策略过滤后，来源集合必须完整覆盖事实引用的全部来源。
                let all_sources_valid = !view.fact.source_event_ids.is_empty()
                    && view.fact.source_event_ids.iter().all(|id| {
                        view.sources
                            .iter()
                            .any(|source| &source.source_event_id == id)
                    });
                if !all_sources_valid {
                    return fallback(
                        MemoryConflictReasonCode::ReReadSourcesInvalidated,
                        "记忆候选与现行记忆存在冲突，但现行记忆的部分来源已失效或不再允许长期记忆，请人工复核。",
                    );
                }
                if !self.is_local_loopback
                    && view.sources.iter().any(|source| {
                        source.content_trust_level == crate::ContentTrustLevel::LocalOnly
                    })
                {
                    return fallback(
                        MemoryConflictReasonCode::ReReadSourcesInvalidated,
                        "记忆候选与现行记忆存在冲突，但现行记忆仅允许本地模型读取，请人工复核。",
                    );
                }
                MemoryCandidateConflictContext::valid(
                    conflict_result.candidate_id.clone(),
                    conflict_result.fact_id.clone(),
                    view.fact.payload.kind(),
                    conflict_result.reason_code,
                    conflict_result.summary.clone(),
                    view.fact.source_event_ids.clone(),
                    crate::summarize_memory_payload(&view.fact.payload, 500),
                )
                .unwrap_or_else(|error| {
                    tracing::warn!(
                        fact_id = conflict_result.fact_id.as_str(),
                        error = %error,
                        "冲突回读上下文构造失败，降级为无效上下文"
                    );
                    fallback(
                        MemoryConflictReasonCode::ReReadSourcesInvalidated,
                        "记忆候选与现行记忆存在冲突，但回读结果不可用，请人工复核。",
                    )
                })
            }
            Ok(None) => fallback(
                MemoryConflictReasonCode::ReReadSourcesInvalidated,
                "记忆候选与现行记忆存在冲突，但现行事实已不存在或不可见，请人工复核。",
            ),
            Err(error) => {
                tracing::warn!(
                    fact_id = conflict_result.fact_id.as_str(),
                    error = %error,
                    "冲突回读失败"
                );
                fallback(
                    MemoryConflictReasonCode::ReReadFailed,
                    "记忆候选与现行记忆存在冲突，但回读现行事实失败，请稍后重试。",
                )
            }
        }
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

        // CMD-009 目标 C：冲突回执（ApproveMemoryCandidate 产生结构化冲突结果）
        // ReplanDecisionNode 的更新已经先应用到状态；记录成功后才回到 Plan。
        // 下一轮 Plan 产生 Outcome 或新回执后不会再次命中本分支。
        if tool_kind == SecretaryToolKind::ApproveMemoryCandidate
            && let Ok(conflict_result) =
                serde_json::from_str::<crate::MemoryCandidateConflictResultV1>(&receipt.result_ref)
            && conflict_result.version == 1
        {
            let already_recorded = business
                .working_context()
                .and_then(|w| w.conflict.as_ref())
                .is_some_and(|c| c.candidate_id == conflict_result.candidate_id);
            return if already_recorded {
                Ok(RouteKey::try_from("continue").unwrap())
            } else {
                // 未形成经回读校验的冲突上下文时 fail-closed，不把原始回执直接交回模型。
                Ok(RouteKey::try_from("finish").unwrap())
            };
        }

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
