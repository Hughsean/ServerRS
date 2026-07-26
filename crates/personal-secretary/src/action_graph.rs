//! Action Graph 领域节点、EffectExecutor 与 Store 端口。
//!
//! Graph 拓扑（约束 4：Effect 只能通过 EffectExecutor 执行一次）：
//! ```text
//! Plan -> Gate(内联) -> L0Execute -> NoAction -> End
//! Plan -> Gate -> Suspend(Approval) -> [resume] -> L0Execute -> NoAction -> End
//! Plan -> Gate -> Suspend(ExternalInput) -> End
//! Plan -> NoAction -> End
//! ```
//!
//! 所有 Effect 只通过 `SecretaryActionEffectExecutor` 执行一次；EffectExecutor 内部
//! 调用 `ActionStoreT::apply_effect`，Store 用 effect_id 做幂等键。

use std::sync::Arc;

use agent_core::graph::{
    AgentNode, EffectEnvelope, EffectError, EffectErrorKind, EffectExecutor, NodeError,
    NodeErrorKind, NodeId, NodeResult, RunContext, UsageDelta,
};
use agent_core::{AgentOutcome, AgentState, AgentUpdate};
use async_trait::async_trait;
use thiserror::Error;

use crate::{
    OwnerResponseDraft, PlannerOutput, RecentEventRef, SecretaryAction,
    SecretaryActionApprovalRequest, SecretaryActionEffect, SecretaryActionProposal,
    SecretaryActionReceipt, SecretaryAgentState, SecretaryAgentUpdate, SecretaryRiskLevel,
    SourceAccountRef, SourceEventId, gate_secretary_action, validate_planner_output,
};

// ===== ActionStoreT 端口（约束 3：CAS + lease fencing）=====

/// 创建 action_run 所需的全部种子数据，封装为结构体避免参数过多。
#[derive(Debug, Clone)]
pub struct ActionRunSeed {
    pub account: SourceAccountRef,
    pub command_source_event_id: SourceEventId,
    pub command_text: String,
    pub conversation_id: String,
    pub occurred_at_unix_secs: i64,
    pub timezone_offset_secs: i64,
    pub recent_events: Vec<RecentEventRef>,
}

/// 领取 suspended run 的完整 CAS 条件。恢复边界必须同时绑定运行、挂起点、
/// Proposal、原 OwnerCommand 与新租约参数，防止错配或旧审批重放。
#[derive(Debug, Clone)]
pub struct SuspendedRunClaim {
    pub run_id: ActionRunId,
    pub checkpoint_id: String,
    pub proposal_id: String,
    pub command_source_event_id: SourceEventId,
    pub worker_id: String,
    pub lease_secs: u64,
    pub now_unix_secs: i64,
}

/// Action 运行存储端口。基础设施层实现，领域层定义。
#[async_trait]
pub trait ActionStoreT: Send + Sync {
    /// 幂等创建 action_run（INSERT IGNORE）。同一 OwnerCommand 重复扫描只运行一次。
    /// 返回是否新建（true=新建，false=已存在）。
    async fn ensure_action_run(
        &self,
        run_id: &ActionRunId,
        seed: &ActionRunSeed,
    ) -> Result<bool, ActionStoreError>;

    /// 领取一个 pending 的 action_run（CAS）。返回领取的运行上下文或 None（无待处理）。
    async fn claim_pending_run(
        &self,
        worker_id: &str,
        lease_secs: u64,
        now_unix_secs: i64,
    ) -> Result<Option<ClaimedActionRun>, ActionStoreError>;

    /// CAS 领取一个等待 Owner 输入的 suspended run，并签发新的恢复租约。
    /// checkpoint_id 必须与 run 当前挂起点一致，防止旧审批恢复新状态。
    async fn claim_suspended_run(
        &self,
        claim: &SuspendedRunClaim,
    ) -> Result<Option<ClaimedActionRun>, ActionStoreError>;

    /// 将持有租约的 running run 标记为 suspended，并释放 Worker 租约。
    /// 完整 Graph Checkpoint 由绑定 run_id 的 CheckpointStore 持久化；这里仅保存索引摘要。
    async fn mark_suspended(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
        checkpoint_json: &str,
    ) -> Result<(), ActionStoreError>;

    /// 加载 Checkpoint。
    async fn load_checkpoint(
        &self,
        run_id: &ActionRunId,
    ) -> Result<Option<String>, ActionStoreError>;

    /// 单次消费 Checkpoint（resume 时调用，CAS 防并发双击）。
    async fn take_checkpoint(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
    ) -> Result<Option<String>, ActionStoreError>;

    /// 查询已提交的 Effect Receipt。Graph 重放时必须先查，避免重复执行真实动作。
    async fn load_effect_receipt(
        &self,
        run_id: &ActionRunId,
        effect_id: &str,
    ) -> Result<Option<SecretaryActionReceipt>, ActionStoreError>;

    /// 应用 Effect（幂等，用 effect_id 去重）。
    /// P0-3 修复：显式传入 run_id，避免误用 proposal_id 作为 run_id。
    async fn apply_effect(
        &self,
        run_id: &ActionRunId,
        effect: &SecretaryActionEffect,
        effect_id: &str,
        result_ref: &str,
        lease_token: &ActionLeaseToken,
    ) -> Result<SecretaryActionReceipt, ActionStoreError>;

    /// 标记运行完成。
    async fn mark_completed(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
        response_draft: Option<&OwnerResponseDraft>,
    ) -> Result<(), ActionStoreError>;

    /// 标记运行失败并设置退避。
    async fn mark_failed(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
        error: &str,
        next_eligible_at_unix_secs: i64,
    ) -> Result<(), ActionStoreError>;

    /// 释放租约（不标记完成/失败，让其他 Worker 可重新领取）。
    async fn release_lease(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
    ) -> Result<(), ActionStoreError>;

    /// 记录审计事件。
    async fn append_audit(
        &self,
        run_id: &ActionRunId,
        event_kind: &str,
        detail_json: &str,
    ) -> Result<(), ActionStoreError>;
}

// ===== 运行标识与租约 =====

macro_rules! action_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ActionStoreError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(ActionStoreError::InvalidData(format!(
                        "{} must not be empty",
                        $field
                    )));
                }
                if value.len() > 36 {
                    return Err(ActionStoreError::InvalidData(format!(
                        "{} must not exceed 36 bytes",
                        $field
                    )));
                }
                Ok(Self(value))
            }

            pub fn generate() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

action_id!(ActionRunId, "action_run_id");
action_id!(ActionLeaseToken, "action_lease_token");

impl ActionRunId {
    /// 从 OwnerCommand 的不可变事件 ID 与 Planner 版本生成稳定的 36 字符 UUID。
    /// 既避免重扫时重复创建，也不会超过数据库 CHAR(36) 边界。
    pub fn for_owner_command(source_event_id: &SourceEventId, planner_version: &str) -> Self {
        let name = format!("{}:{planner_version}", source_event_id.as_str());
        Self(uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, name.as_bytes()).to_string())
    }
}

/// 领取到的 action_run 运行上下文。
#[derive(Debug, Clone)]
pub struct ClaimedActionRun {
    pub run_id: ActionRunId,
    pub lease_token: ActionLeaseToken,
    pub account: SourceAccountRef,
    pub command_source_event_id: SourceEventId,
    pub command_text: String,
    pub conversation_id: String,
    pub occurred_at_unix_secs: i64,
    pub timezone_offset_secs: i64,
    pub recent_events: Vec<RecentEventRef>,
}

// ===== 错误类型（约束 8：错误分类不能全映射为 UnknownCommit）=====

#[derive(Debug, Error)]
pub enum ActionStoreError {
    #[error("invalid action data: {0}")]
    InvalidData(String),
    #[error("action store is unavailable")]
    Unavailable,
    #[error("action database operation failed: {0}")]
    Database(String),
    #[error("action lease ownership was lost")]
    LeaseLost,
    /// 约束 8：只有"可能已提交但没拿到结果"才是 UnknownCommit。
    #[error("action effect may have been committed but result is unknown: {0}")]
    UnknownCommit(String),
}

impl From<crate::InboundEventStoreError> for ActionStoreError {
    fn from(error: crate::InboundEventStoreError) -> Self {
        match error {
            crate::InboundEventStoreError::InvalidData(msg) => Self::InvalidData(msg),
            crate::InboundEventStoreError::Unavailable => Self::Unavailable,
            crate::InboundEventStoreError::Database(msg) => Self::Database(msg),
            crate::InboundEventStoreError::LeaseLost => Self::LeaseLost,
        }
    }
}

impl ActionStoreError {
    /// 把存储错误映射为 Effect 错误分类（约束 8）。
    #[allow(dead_code)]
    pub fn to_effect_error(self) -> EffectError {
        match self {
            Self::InvalidData(_) | Self::LeaseLost => {
                EffectError::with_source(EffectErrorKind::Permanent, self)
            }
            Self::Unavailable | Self::Database(_) => {
                EffectError::with_source(EffectErrorKind::UnknownCommit, self)
            }
            Self::UnknownCommit(_) => {
                EffectError::with_source(EffectErrorKind::UnknownCommit, self)
            }
        }
    }
}

// ===== 运行时上下文（注入节点，不污染 AgentState）=====

/// 一次 Action 运行的上下文，由 Worker 领取后注入 PlanNode。
/// 不存入 AgentState，避免状态机膨胀（约束 2）。
#[derive(Debug, Clone)]
pub struct ActionRunContext {
    pub account: SourceAccountRef,
    pub command_source_event_id: SourceEventId,
    pub command_text: String,
    pub conversation_id: String,
    pub occurred_at_unix_secs: i64,
    pub timezone_offset_secs: i64,
    pub now_unix_secs: i64,
    pub lease_token: ActionLeaseToken,
}

// ===== Graph 节点 =====

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
            id: NodeId::try_from("plan").map_err(|e| ActionGraphError(e.to_string()))?,
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
            let query = crate::EventQuery {
                account: self.context.account.clone(),
                conversation: Some(
                    crate::ConversationRef::new(
                        crate::ConversationKind::OwnerControl,
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
                .map(|r| crate::PlannerRetrievedExcerpt {
                    source_event_id: r.source_event_id,
                    excerpt: r.excerpt,
                    occurred_at_unix_secs: r.occurred_at_unix_secs,
                    actor_id: r.actor.id,
                })
                .collect()
        } else {
            Vec::new()
        };
        let input = crate::PlannerInput {
            account: self.context.account.clone(),
            command: crate::PlannerCommandEvent {
                source_event_id: self.context.command_source_event_id.clone(),
                conversation: crate::ConversationRef::new(
                    crate::ConversationKind::OwnerControl,
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
            id: NodeId::try_from("l0_execute").map_err(|e| ActionGraphError(e.to_string()))?,
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
            id: NodeId::try_from("build_response").map_err(|e| ActionGraphError(e.to_string()))?,
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
        let mut source_ids: Vec<SourceEventId> = Vec::new();
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
        let segments = vec![crate::ResponseSegment::Summary { text: summary }];
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
            id: NodeId::try_from("no_action").map_err(|e| ActionGraphError(e.to_string()))?,
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

// ===== EffectExecutor（带 run_id + lease_token + Retriever）=====

/// Secretary Action Effect 执行器。
/// P0-3 修复：显式持有 run_id，避免误用 proposal_id。
/// P0-4 修复：根据 Action 类型调用 Retriever 生成真实查询结果，
/// 再调 ActionStoreT::apply_effect 持久化幂等 Receipt。
pub struct SecretaryActionEffectExecutor {
    store: Arc<dyn ActionStoreT>,
    run_id: ActionRunId,
    lease_token: ActionLeaseToken,
    retriever: Option<Arc<crate::RetrieverUseCase>>,
    account: SourceAccountRef,
    now_unix_secs: i64,
}

impl SecretaryActionEffectExecutor {
    pub fn new(
        store: Arc<dyn ActionStoreT>,
        run_id: ActionRunId,
        lease_token: ActionLeaseToken,
        retriever: Option<Arc<crate::RetrieverUseCase>>,
        account: SourceAccountRef,
        now_unix_secs: i64,
    ) -> Self {
        Self {
            store,
            run_id,
            lease_token,
            retriever,
            account,
            now_unix_secs,
        }
    }

    /// 根据 Action 类型执行真实查询，返回结果摘要作为 result_ref。
    /// P0-4 修复：Effect 不再只写 executed:{effect_id}，而是调用 Retriever 生成真实结果。
    async fn execute_action(&self, action: &SecretaryAction) -> Result<String, EffectError> {
        let retriever = self.retriever.as_ref().ok_or_else(|| {
            EffectError::new(
                EffectErrorKind::Permanent,
                "Retriever 未注入，无法执行查询型 Action",
            )
        })?;
        match action {
            SecretaryAction::SearchRecentEvents { query, limit } => {
                let event_query = crate::EventQuery {
                    account: self.account.clone(),
                    conversation: None,
                    actor_id: None,
                    thread_id: None,
                    since_unix_secs: Some(self.now_unix_secs - 86_400),
                    until_unix_secs: Some(self.now_unix_secs),
                    query_text: Some(query.clone()),
                    limit: *limit,
                };
                let results = retriever
                    .search_events(&event_query, false)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                Ok(format_event_results(&results))
            }
            SecretaryAction::ReadSourceEvent { source_event_id } => {
                let detail = retriever
                    .read_source_event(source_event_id, &self.account)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                match detail {
                    Some(d) => Ok(format!(
                        "事件 {} | {} | {} | 摘录: {}",
                        d.source_event_id.as_str(),
                        d.actor.id,
                        d.occurred_at_unix_secs,
                        d.normalized_text.chars().take(120).collect::<String>(),
                    )),
                    None => Ok(format!("未找到事件 {}", source_event_id.as_str())),
                }
            }
            SecretaryAction::SearchEventThreads { query, limit } => {
                let results = retriever
                    .search_threads(&self.account, query, *limit)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                Ok(format!("搜索到 {} 个线程", results.len()))
            }
            SecretaryAction::ResolveReference { expression } => {
                let context = crate::ReferenceContext {
                    account: self.account.clone(),
                    current_conversation: None,
                    current_thread_id: None,
                    recent_events: Vec::new(),
                    now_unix_secs: self.now_unix_secs,
                    timezone: "UTC".into(),
                };
                let resolution = retriever
                    .resolve_reference(&self.account, expression, &context)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                Ok(if resolution.ambiguous {
                    format!("指代歧义：{}", resolution.evidence)
                } else {
                    format!("指代已解析：{}", resolution.evidence)
                })
            }
            SecretaryAction::ListUpcomingItems { horizon_secs } => {
                let items = retriever
                    .list_upcoming(&self.account, *horizon_secs)
                    .await
                    .map_err(|e| EffectError::new(EffectErrorKind::Transient, e.to_string()))?;
                Ok(format!("查到 {} 个即将到期事项", items.len()))
            }
            SecretaryAction::DraftReminder { text, .. } => Ok(format!(
                "已起草提醒：{}",
                text.chars().take(50).collect::<String>()
            )),
            SecretaryAction::AskOwnerClarification { question } => Ok(format!(
                "已向 Owner 提问：{}",
                question.chars().take(50).collect::<String>()
            )),
            other => Err(EffectError::new(
                EffectErrorKind::Permanent,
                format!("本批不支持执行 Action: {:?}", other.kind()),
            )),
        }
    }
}

#[async_trait]
impl EffectExecutor<SecretaryActionEffect> for SecretaryActionEffectExecutor {
    async fn execute(
        &self,
        envelope: &EffectEnvelope<SecretaryActionEffect>,
        _context: &RunContext,
    ) -> Result<SecretaryActionReceipt, EffectError> {
        if let Some(receipt) = self
            .store
            .load_effect_receipt(&self.run_id, &envelope.id.to_string())
            .await
            .map_err(ActionStoreError::to_effect_error)?
        {
            return Ok(receipt);
        }
        // 未命中既有 Receipt 才执行真实 Action；Store 提交时再次处理并发竞争。
        let result_ref = self
            .execute_action(&envelope.effect.proposal.action)
            .await?;
        self.store
            .apply_effect(
                &self.run_id,
                &envelope.effect,
                &envelope.id.to_string(),
                &result_ref,
                &self.lease_token,
            )
            .await
            .map_err(|e| e.to_effect_error())
    }
}

/// 格式化事件检索结果为有界摘要（含来源、时间、Actor、摘录、命中数）。
/// 最多展示前 3 条事件详情，超过时标记截断。
fn format_event_results(results: &[crate::EventSearchResult]) -> String {
    let total = results.len();
    if total == 0 {
        return "未找到匹配事件".into();
    }
    let show = total.min(3);
    let parts: Vec<String> = results[..show]
        .iter()
        .map(|r| {
            format!(
                "{} | {} | {}",
                r.source_event_id.as_str(),
                r.actor.id,
                r.excerpt.chars().take(80).collect::<String>()
            )
        })
        .collect();
    let truncation = if total > show {
        format!("，仅展示前 {show} 条")
    } else {
        String::new()
    };
    format!("命中 {total} 条{truncation}: {}", parts.join("; "))
}

// ===== Graph 装配 =====

#[derive(Debug, Error)]
#[error("action graph error: {0}")]
pub struct ActionGraphError(String);

/// 装配好的 Action Graph Runtime。
pub type ActionGraphRuntime = agent_core::graph::GraphRuntime<SecretaryAgentState>;

/// 装配 Action Graph。
///
/// 拓扑：`Plan -> (Gate 内联) -> L0Execute -> NoAction -> End`
/// 以及 `Plan -> Suspend -> End`（挂起后由 Checkpoint 恢复）。
///
/// BuildResponse 逻辑由 Effect receipt 驱动，在应用层 run_once 中组装 OwnerResponseDraft。
pub fn build_action_graph(
    planner: Arc<dyn crate::ActionPlannerT>,
    retriever: Option<Arc<crate::RetrieverUseCase>>,
    context: Arc<ActionRunContext>,
    checkpoint_store: Arc<dyn agent_core::graph::CheckpointStore<SecretaryAgentState>>,
    effect_executor: Arc<SecretaryActionEffectExecutor>,
) -> Result<ActionGraphRuntime, ActionGraphError> {
    use agent_core::graph::{GraphDefinition, GraphId, GraphPolicy, TransitionRule};
    use std::num::NonZeroU32;

    let mut graph = GraphDefinition::new(GraphId::try_from("secretary_action").unwrap());
    graph
        .add_node(Arc::new(PlanNode::new(
            planner,
            retriever,
            Arc::clone(&context),
        )?))
        .map_err(|e| ActionGraphError(e.to_string()))?;
    graph
        .add_node(Arc::new(L0ExecuteNode::new()?))
        .map_err(|e| ActionGraphError(e.to_string()))?;
    graph
        .add_node(Arc::new(BuildResponseNode::new(Arc::clone(&context))?))
        .map_err(|e| ActionGraphError(e.to_string()))?;
    let plan = NodeId::try_from("plan").unwrap();
    let l0 = NodeId::try_from("l0_execute").unwrap();
    let build = NodeId::try_from("build_response").unwrap();

    graph.set_entry(plan.clone());
    graph
        .set_transition(plan.clone(), TransitionRule::Goto(l0.clone()))
        .unwrap();
    graph
        .set_transition(l0.clone(), TransitionRule::Goto(build.clone()))
        .unwrap();
    graph
        .set_transition(build.clone(), TransitionRule::End)
        .unwrap();

    let compiled = graph
        .compile(GraphPolicy::new(NonZeroU32::new(16).unwrap()))
        .unwrap();
    Ok(
        ActionGraphRuntime::with_effect_executor(compiled, effect_executor)
            .with_checkpoint_store(checkpoint_store),
    )
}

/// 判定 Action 风险等级是否允许在 L0 路径直接执行（约束 5）。
pub fn is_l0_direct_execute(risk: SecretaryRiskLevel) -> bool {
    matches!(
        risk,
        SecretaryRiskLevel::L0ReadOnly | SecretaryRiskLevel::L1Reversible
    )
}

/// 退避时间计算（约束 3：在 Rust 中饱和计算）。
/// 指数退避：base * 2^(attempt-1)，上限 max_ms。
pub fn backoff_ms(attempt: u32, base_ms: u64, max_ms: u64) -> u64 {
    if attempt == 0 {
        return base_ms;
    }
    let exponent = attempt.saturating_sub(1).min(10);
    let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
    base_ms.saturating_mul(multiplier).min(max_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_first_attempt_is_base() {
        assert_eq!(backoff_ms(1, 500, 10_000), 500);
    }

    #[test]
    fn backoff_doubles_each_attempt() {
        assert_eq!(backoff_ms(2, 500, 10_000), 1000);
        assert_eq!(backoff_ms(3, 500, 10_000), 2000);
    }

    #[test]
    fn backoff_capped_at_max() {
        assert_eq!(backoff_ms(10, 500, 10_000), 10_000);
    }

    #[test]
    fn backoff_saturates_on_huge_attempt() {
        assert_eq!(backoff_ms(u32::MAX, 500, 10_000), 10_000);
    }

    #[test]
    fn l0_readonly_is_direct_execute() {
        assert!(is_l0_direct_execute(SecretaryRiskLevel::L0ReadOnly));
    }

    #[test]
    fn l1_reversible_is_direct_execute() {
        assert!(is_l0_direct_execute(SecretaryRiskLevel::L1Reversible));
    }

    #[test]
    fn l2_impactful_not_direct_execute() {
        assert!(!is_l0_direct_execute(SecretaryRiskLevel::L2Impactful));
    }

    #[test]
    fn l3_external_not_direct_execute() {
        assert!(!is_l0_direct_execute(
            SecretaryRiskLevel::L3ExternalSideEffect
        ));
    }

    #[test]
    fn action_run_id_rejects_empty() {
        assert!(ActionRunId::new("").is_err());
        assert!(ActionRunId::new("  ").is_err());
    }

    #[test]
    fn action_run_id_accepts_non_empty() {
        assert!(ActionRunId::new("run-1").is_ok());
    }

    #[test]
    fn action_ids_reject_database_truncation() {
        assert!(ActionRunId::new("x".repeat(37)).is_err());
        assert!(ActionLeaseToken::new("x".repeat(37)).is_err());
    }

    #[test]
    fn owner_command_run_id_is_stable_uuid_and_version_scoped() {
        let source = SourceEventId::new("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let first = ActionRunId::for_owner_command(&source, "v1");
        let repeated = ActionRunId::for_owner_command(&source, "v1");
        let upgraded = ActionRunId::for_owner_command(&source, "v2");
        assert_eq!(first, repeated);
        assert_ne!(first, upgraded);
        assert_eq!(first.as_str().len(), 36);
        assert!(uuid::Uuid::parse_str(first.as_str()).is_ok());
    }

    #[test]
    fn action_lease_token_generates_uuid() {
        let token = ActionLeaseToken::generate();
        assert!(!token.as_str().is_empty());
    }

    #[test]
    fn invalid_data_maps_to_permanent_effect_error() {
        let error = ActionStoreError::InvalidData("test".into());
        assert_eq!(error.to_effect_error().kind(), EffectErrorKind::Permanent);
    }

    #[test]
    fn lease_lost_maps_to_permanent_effect_error() {
        let error = ActionStoreError::LeaseLost;
        assert_eq!(error.to_effect_error().kind(), EffectErrorKind::Permanent);
    }

    #[test]
    fn database_error_maps_to_unknown_commit() {
        let error = ActionStoreError::Database("connection lost".into());
        assert_eq!(
            error.to_effect_error().kind(),
            EffectErrorKind::UnknownCommit
        );
    }

    #[test]
    fn unavailable_maps_to_unknown_commit() {
        let error = ActionStoreError::Unavailable;
        assert_eq!(
            error.to_effect_error().kind(),
            EffectErrorKind::UnknownCommit
        );
    }

    #[test]
    fn unknown_commit_maps_to_unknown_commit() {
        let error = ActionStoreError::UnknownCommit("maybe committed".into());
        assert_eq!(
            error.to_effect_error().kind(),
            EffectErrorKind::UnknownCommit
        );
    }
}
