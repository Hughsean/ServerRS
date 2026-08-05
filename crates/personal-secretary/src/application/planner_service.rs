//! Action Planner 用例编排。
//!
//! Worker 调用 `run_once`：领取 action_run -> 构建 Graph 上下文 -> 运行 Graph ->
//! 标记完成/失败/释放租约。Graph 内部由 PlanNode 调用 Planner、Gate 路由、EffectExecutor 执行。
//!
//! 约束 3/4/8：
//! - 领取用 CAS，RowsAffected==1
//! - 所有进度提交验证 lease_token
//! - 退避时间在 Rust 中饱和计算
//! - 错误分类不能全映射为 UnknownCommit

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use agent_core::AgentState;
use agent_core::graph::{CheckpointStore, GraphExecutionResult, RunBudget};
use thiserror::Error;
use tracing::{info, warn};

use crate::{
    ActionLeaseToken, ActionRunContext, ActionRunId, ActionRunSeed, ActionStoreError, ActionStoreT,
    ClaimedActionRun, Clock, FollowUpControlUseCase, OwnerResponseDraft,
    SecretaryActionEffectExecutor, SecretaryAgentState, SuspendedRunClaim, SystemClock, backoff_ms,
    build_action_graph,
};

/// Planner 用例错误。
#[derive(Debug, Error)]
pub enum PlannerUseCaseError {
    #[error(transparent)]
    Store(#[from] ActionStoreError),
    #[error("graph run failed: {0}")]
    GraphRun(String),
    #[error("planner timed out")]
    Timeout,
}

impl PlannerUseCaseError {
    /// 判定错误是否可重试（约束 8）。
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Store(ActionStoreError::Unavailable)
            | Self::Store(ActionStoreError::Database(_)) => true,
            Self::Timeout => true,
            Self::GraphRun(_) => true,
            Self::Store(ActionStoreError::InvalidData(_))
            | Self::Store(ActionStoreError::LeaseLost)
            | Self::Store(ActionStoreError::UnknownCommit(_)) => false,
        }
    }
}

/// 单次运行报告。
#[derive(Debug, Clone, Default)]
pub struct PlannerRunReport {
    pub run_id: Option<String>,
    pub completed: bool,
    pub suspended: bool,
    pub failed: bool,
    pub error: Option<String>,
    /// 挂起时的 CheckpointId，用于后续 resume。
    pub checkpoint_id: Option<String>,
    /// 挂起请求对应的 ProposalId；Owner 恢复输入必须原样回传。
    pub proposal_id: Option<String>,
}

/// 按业务 ActionRun 创建持久化 Graph CheckpointStore 的应用端口。
pub trait ActionCheckpointStoreFactoryT: Send + Sync {
    fn for_run(&self, action_run_id: &ActionRunId)
    -> Arc<dyn CheckpointStore<SecretaryAgentState>>;
}

struct SharedCheckpointStoreFactory {
    store: Arc<dyn CheckpointStore<SecretaryAgentState>>,
}

impl ActionCheckpointStoreFactoryT for SharedCheckpointStoreFactory {
    fn for_run(
        &self,
        _action_run_id: &ActionRunId,
    ) -> Arc<dyn CheckpointStore<SecretaryAgentState>> {
        Arc::clone(&self.store)
    }
}

/// Planner 用例。编排 Graph 运行与 Store 提交。
/// P0-2 修复：持有 RetrieverUseCase，让 PlanNode 能检索数据库证据。
/// CheckpointStore 由外层适配器工厂按 ActionRunId 提供，应用层不感知数据库类型。
pub struct PlannerUseCase {
    store: Arc<dyn ActionStoreT>,
    planner: Arc<dyn crate::ActionPlannerT>,
    retriever: Option<Arc<crate::RetrieverUseCase>>,
    notification_policy: Option<Arc<crate::NotificationPolicyUseCase>>,
    agenda: Option<Arc<crate::AgendaUseCase>>,
    memory: Option<Arc<crate::MemoryUseCase>>,
    thread_control: Option<Arc<crate::ThreadControlUseCase>>,
    follow_up_control: Option<Arc<FollowUpControlUseCase>>,
    response_expectation_control: Option<Arc<crate::ResponseExpectationControlUseCase>>,
    memory_candidate: Option<Arc<crate::MemoryCandidateUseCase>>,
    memory_candidate_control: Option<Arc<crate::MemoryCandidateControlUseCase>>,
    thread_link_review: Option<Arc<crate::ThreadLinkReviewUseCase>>,
    checkpoint_store_factory: Arc<dyn ActionCheckpointStoreFactoryT>,
    clock: Arc<dyn Clock>,
    /// 当前 LLM 端点是否已验证为本地回环。注入 ActionRunContext 供 PlanNode 和 Planner 使用。
    is_local_loopback: bool,
    lease_secs: u64,
    max_steps: u32,
    deadline_ms: u64,
}

impl PlannerUseCase {
    pub fn new(
        store: Arc<dyn ActionStoreT>,
        planner: Arc<dyn crate::ActionPlannerT>,
        checkpoint_store: Arc<dyn CheckpointStore<SecretaryAgentState>>,
        lease_secs: u64,
    ) -> Self {
        Self {
            store,
            planner,
            retriever: None,
            notification_policy: None,
            agenda: None,
            memory: None,
            thread_control: None,
            follow_up_control: None,
            response_expectation_control: None,
            memory_candidate: None,
            memory_candidate_control: None,
            thread_link_review: None,
            checkpoint_store_factory: Arc::new(SharedCheckpointStoreFactory {
                store: checkpoint_store,
            }),
            clock: Arc::new(SystemClock),
            is_local_loopback: false,
            lease_secs,
            max_steps: 16,
            deadline_ms: 30_000,
        }
    }

    pub fn with_checkpoint_store_factory(
        mut self,
        factory: Arc<dyn ActionCheckpointStoreFactoryT>,
    ) -> Self {
        self.checkpoint_store_factory = factory;
        self
    }

    /// P0-2 修复：注入 RetrieverUseCase，让 PlanNode 检索数据库证据。
    pub fn with_retriever(mut self, retriever: Arc<crate::RetrieverUseCase>) -> Self {
        self.retriever = Some(retriever);
        self
    }

    pub fn with_notification_policy(
        mut self,
        notification_policy: Arc<crate::NotificationPolicyUseCase>,
    ) -> Self {
        self.notification_policy = Some(notification_policy);
        self
    }

    pub fn with_agenda(mut self, agenda: Arc<crate::AgendaUseCase>) -> Self {
        self.agenda = Some(agenda);
        self
    }

    pub fn with_memory(mut self, memory: Arc<crate::MemoryUseCase>) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn with_thread_control(mut self, thread_control: Arc<crate::ThreadControlUseCase>) -> Self {
        self.thread_control = Some(thread_control);
        self
    }

    pub fn with_follow_up_control(
        mut self,
        follow_up_control: Arc<FollowUpControlUseCase>,
    ) -> Self {
        self.follow_up_control = Some(follow_up_control);
        self
    }

    pub fn with_response_expectation_control(
        mut self,
        response_expectation_control: Arc<crate::ResponseExpectationControlUseCase>,
    ) -> Self {
        self.response_expectation_control = Some(response_expectation_control);
        self
    }

    pub fn with_memory_candidate(
        mut self,
        memory_candidate: Arc<crate::MemoryCandidateUseCase>,
    ) -> Self {
        self.memory_candidate = Some(memory_candidate);
        self
    }

    pub fn with_memory_candidate_control(
        mut self,
        memory_candidate_control: Arc<crate::MemoryCandidateControlUseCase>,
    ) -> Self {
        self.memory_candidate_control = Some(memory_candidate_control);
        self
    }

    pub fn with_thread_link_review(
        mut self,
        thread_link_review: Arc<crate::ThreadLinkReviewUseCase>,
    ) -> Self {
        self.thread_link_review = Some(thread_link_review);
        self
    }

    /// CTX-002 修复：注入已验证的本地回环标志，控制 local_only 内容是否对 LLM 可见。
    pub fn with_loopback(mut self, is_local_loopback: bool) -> Self {
        self.is_local_loopback = is_local_loopback;
        self
    }

    pub fn with_clock(
        store: Arc<dyn ActionStoreT>,
        planner: Arc<dyn crate::ActionPlannerT>,
        checkpoint_store: Arc<dyn CheckpointStore<SecretaryAgentState>>,
        lease_secs: u64,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            store,
            planner,
            retriever: None,
            notification_policy: None,
            agenda: None,
            memory: None,
            thread_control: None,
            follow_up_control: None,
            response_expectation_control: None,
            memory_candidate: None,
            memory_candidate_control: None,
            thread_link_review: None,
            checkpoint_store_factory: Arc::new(SharedCheckpointStoreFactory {
                store: checkpoint_store,
            }),
            clock,
            is_local_loopback: false,
            lease_secs,
            max_steps: 16,
            deadline_ms: 30_000,
        }
    }

    /// 领取并运行一个 action_run。返回报告。
    /// Worker 在循环中调用此方法；返回 Ok(None) 表示无待处理。
    pub async fn run_once(
        &self,
        worker_id: &str,
    ) -> Result<Option<PlannerRunReport>, PlannerUseCaseError> {
        let now = self.clock.now_unix_secs();
        let Some(claimed) = self
            .store
            .claim_pending_run(worker_id, self.lease_secs, now)
            .await?
        else {
            return Ok(None);
        };
        let run_id = claimed.run_id.clone();
        let lease_token = claimed.lease_token.clone();
        let attempt = 0; // 首次领取 attempt=0；租约过期回收时已递增
        let report = self.execute_claimed(claimed).await;
        match report {
            Ok(report) => Ok(Some(report)),
            Err(error) => {
                // P0 修复：逐 Run 失败退避。调用 handle_failure 更新对应 Run 状态。
                if let Err(handle_error) = self
                    .handle_failure(&run_id, &lease_token, &error, attempt + 1)
                    .await
                {
                    warn!(
                        run_id = run_id.as_str(),
                        original_error = %error,
                        handle_error = %handle_error,
                        "handle_failure 也失败了，租约将在过期后被回收"
                    );
                }
                Err(error)
            }
        }
    }

    /// 幂等创建 action_run（外部触发，如 OwnerCommand 入站时）。
    pub async fn ensure_action_run(
        &self,
        run_id: &ActionRunId,
        seed: &ActionRunSeed,
    ) -> Result<bool, PlannerUseCaseError> {
        Ok(self.store.ensure_action_run(run_id, seed).await?)
    }

    /// 执行已领取的 action_run。
    async fn execute_claimed(
        &self,
        claimed: ClaimedActionRun,
    ) -> Result<PlannerRunReport, PlannerUseCaseError> {
        let run_id = claimed.run_id.clone();
        let lease_token = claimed.lease_token.clone();
        let context = Arc::new(ActionRunContext {
            account: claimed.account.clone(),
            command_source_event_id: claimed.command_source_event_id.clone(),
            command_text: claimed.command_text.clone(),
            conversation_id: claimed.conversation_id.clone(),
            occurred_at_unix_secs: claimed.occurred_at_unix_secs,
            timezone_offset_secs: claimed.timezone_offset_secs,
            timezone: claimed.timezone.clone(),
            now_unix_secs: self.clock.now_unix_secs(),
            lease_token: lease_token.clone(),
            is_local_loopback: self.is_local_loopback,
        });

        let mut effect_executor = SecretaryActionEffectExecutor::new(
            Arc::clone(&self.store),
            claimed.run_id.clone(),
            lease_token.clone(),
            self.retriever.clone(),
            claimed.account.clone(),
            self.clock.now_unix_secs(),
        )
        .with_loopback(self.is_local_loopback)
        .with_reference_context(
            claimed.command_source_event_id.clone(),
            claimed.recent_events.clone(),
        );
        if let Some(notification_policy) = &self.notification_policy {
            effect_executor = effect_executor.with_notification_policy(
                Arc::clone(notification_policy),
                claimed.command_source_event_id.clone(),
            );
        }
        if let Some(agenda) = &self.agenda {
            effect_executor = effect_executor
                .with_agenda(Arc::clone(agenda), claimed.command_source_event_id.clone());
        }
        if let Some(memory) = &self.memory {
            effect_executor = effect_executor
                .with_memory(Arc::clone(memory), claimed.command_source_event_id.clone());
        }
        if let Some(thread_control) = &self.thread_control {
            effect_executor = effect_executor.with_thread_control(
                Arc::clone(thread_control),
                claimed.command_source_event_id.clone(),
            );
        }
        if let Some(follow_up_control) = &self.follow_up_control {
            effect_executor = effect_executor.with_follow_up_control(
                Arc::clone(follow_up_control),
                claimed.command_source_event_id.clone(),
            );
        }
        if let Some(response_expectation_control) = &self.response_expectation_control {
            effect_executor = effect_executor.with_response_expectation_control(
                Arc::clone(response_expectation_control),
                claimed.command_source_event_id.clone(),
            );
        }
        if let Some(memory_candidate) = &self.memory_candidate {
            effect_executor = effect_executor.with_memory_candidate(Arc::clone(memory_candidate));
        }
        if let Some(memory_candidate_control) = &self.memory_candidate_control {
            effect_executor = effect_executor.with_memory_candidate_control(
                Arc::clone(memory_candidate_control),
                claimed.command_source_event_id.clone(),
            );
        }
        if let Some(thread_link_review) = &self.thread_link_review {
            effect_executor =
                effect_executor.with_thread_link_review(Arc::clone(thread_link_review));
        }
        let effect_executor = Arc::new(effect_executor);

        let checkpoint_store = self.checkpoint_store_factory.for_run(&run_id);

        let graph = build_action_graph(
            Arc::clone(&self.planner),
            self.retriever.clone(),
            Arc::clone(&context),
            checkpoint_store,
            effect_executor,
            self.memory.clone(),
        )
        .map_err(|e| PlannerUseCaseError::GraphRun(e.to_string()))?;

        let state = SecretaryAgentState::new(
            &claimed.command_text,
            Vec::new(),
            vec![claimed.command_source_event_id.clone()],
            claimed.recent_events.clone(),
        )
        .map_err(|e| PlannerUseCaseError::GraphRun(e.to_string()))?;

        let budget = RunBudget::new(
            NonZeroU32::new(self.max_steps).unwrap(),
            Duration::from_millis(self.deadline_ms),
        );

        let result = graph
            .run_checkpointed(AgentState::new(state), budget)
            .await
            .map_err(|e| PlannerUseCaseError::GraphRun(e.to_string()))?;

        match result {
            GraphExecutionResult::Completed(completed) => {
                // 组装响应草稿
                let draft = self.build_response_draft(completed.state.business());
                self.store
                    .mark_completed(&run_id, &lease_token, draft.as_ref())
                    .await?;
                self.store.append_audit(&run_id, "completed", "{}").await?;
                Ok(PlannerRunReport {
                    run_id: Some(run_id.as_str().to_owned()),
                    completed: true,
                    ..Default::default()
                })
            }
            GraphExecutionResult::Suspended(suspended) => {
                // 挂起等待 Owner 审批/澄清。Checkpoint 已由 Runtime 保存。
                let checkpoint_id = suspended.checkpoint().id();
                let proposal_id = suspended.checkpoint().suspend().data.proposal_id.clone();
                let checkpoint_json = serde_json::json!({
                    "checkpoint_id": checkpoint_id.to_string(),
                    "proposal_id": proposal_id,
                    "reason": suspended.checkpoint().suspend().reason,
                    "next_node": suspended.checkpoint().position().next_node().as_str(),
                })
                .to_string();
                self.store
                    .mark_suspended(&run_id, &lease_token, &checkpoint_json)
                    .await?;
                self.store
                    .append_audit(&run_id, "suspended", &checkpoint_json)
                    .await?;
                info!(
                    run_id = run_id.as_str(),
                    checkpoint = %checkpoint_id,
                    "action run suspended awaiting owner input"
                );
                Ok(PlannerRunReport {
                    run_id: Some(run_id.as_str().to_owned()),
                    suspended: true,
                    checkpoint_id: Some(checkpoint_id.to_string()),
                    proposal_id: Some(proposal_id),
                    ..Default::default()
                })
            }
        }
    }

    /// 从 AgentState 组装有界响应草稿。
    /// 优先使用 Graph 已设置的 response_draft（由 BuildResponseNode 产生），
    /// 否则回退到共享响应构造函数（P0 修复：统一响应入口）。
    fn build_response_draft(&self, state: &SecretaryAgentState) -> Option<OwnerResponseDraft> {
        if let Some(draft) = state.response_draft() {
            return Some(draft.clone());
        }
        crate::build_action_response_draft(
            state.last_receipt(),
            state.evidence_source_event_ids().to_vec(),
            self.clock.now_unix_secs(),
        )
        .ok()
    }

    /// 查询指定账号等待 Owner 审批的运行；调用方负责处理零项与多项歧义。
    pub async fn list_suspended_runs(
        &self,
        account: &crate::SourceAccountRef,
        limit: u32,
    ) -> Result<Vec<crate::SuspendedActionRun>, PlannerUseCaseError> {
        self.store
            .list_suspended_runs(account, limit)
            .await
            .map_err(PlannerUseCaseError::from)
    }

    /// P0-5: 恢复挂起的 action_run。Owner 审批后调用。
    /// 加载 Checkpoint、CAS 单次消费、恢复 Graph 运行、标记完成。
    pub async fn resume_run(
        &self,
        run_id: &ActionRunId,
        checkpoint_id: &str,
        resume_input: crate::SecretaryActionResumeInput,
    ) -> Result<PlannerRunReport, PlannerUseCaseError> {
        use agent_core::graph::CheckpointId;
        let cid: CheckpointId = checkpoint_id
            .parse()
            .map_err(|e| PlannerUseCaseError::GraphRun(format!("invalid checkpoint_id: {e}")))?;
        let claimed = self
            .store
            .claim_suspended_run(&SuspendedRunClaim {
                run_id: run_id.clone(),
                checkpoint_id: checkpoint_id.to_owned(),
                proposal_id: resume_input.proposal_id.clone(),
                command_source_event_id: resume_input.command_source_event_id.clone(),
                worker_id: "owner-resume".into(),
                lease_secs: self.lease_secs,
                now_unix_secs: self.clock.now_unix_secs(),
            })
            .await?
            .ok_or(ActionStoreError::LeaseLost)?;
        let checkpoint_store = self.checkpoint_store_factory.for_run(run_id);
        let mut effect_executor = SecretaryActionEffectExecutor::new(
            Arc::clone(&self.store),
            run_id.clone(),
            claimed.lease_token.clone(),
            self.retriever.clone(),
            claimed.account.clone(),
            self.clock.now_unix_secs(),
        )
        .with_loopback(self.is_local_loopback)
        .with_reference_context(
            claimed.command_source_event_id.clone(),
            claimed.recent_events.clone(),
        );
        if let Some(notification_policy) = &self.notification_policy {
            effect_executor = effect_executor.with_notification_policy(
                Arc::clone(notification_policy),
                claimed.command_source_event_id.clone(),
            );
        }
        if let Some(agenda) = &self.agenda {
            effect_executor = effect_executor
                .with_agenda(Arc::clone(agenda), claimed.command_source_event_id.clone());
        }
        if let Some(memory) = &self.memory {
            effect_executor = effect_executor
                .with_memory(Arc::clone(memory), claimed.command_source_event_id.clone());
        }
        if let Some(thread_control) = &self.thread_control {
            effect_executor = effect_executor.with_thread_control(
                Arc::clone(thread_control),
                claimed.command_source_event_id.clone(),
            );
        }
        if let Some(follow_up_control) = &self.follow_up_control {
            effect_executor = effect_executor.with_follow_up_control(
                Arc::clone(follow_up_control),
                claimed.command_source_event_id.clone(),
            );
        }
        if let Some(response_expectation_control) = &self.response_expectation_control {
            effect_executor = effect_executor.with_response_expectation_control(
                Arc::clone(response_expectation_control),
                claimed.command_source_event_id.clone(),
            );
        }
        if let Some(memory_candidate) = &self.memory_candidate {
            effect_executor = effect_executor.with_memory_candidate(Arc::clone(memory_candidate));
        }
        if let Some(memory_candidate_control) = &self.memory_candidate_control {
            effect_executor = effect_executor.with_memory_candidate_control(
                Arc::clone(memory_candidate_control),
                claimed.command_source_event_id.clone(),
            );
        }
        if let Some(thread_link_review) = &self.thread_link_review {
            effect_executor =
                effect_executor.with_thread_link_review(Arc::clone(thread_link_review));
        }
        let effect_executor = Arc::new(effect_executor);
        let context = Arc::new(ActionRunContext {
            account: claimed.account,
            command_source_event_id: claimed.command_source_event_id,
            command_text: claimed.command_text,
            conversation_id: claimed.conversation_id,
            occurred_at_unix_secs: claimed.occurred_at_unix_secs,
            timezone_offset_secs: claimed.timezone_offset_secs,
            timezone: claimed.timezone.clone(),
            now_unix_secs: self.clock.now_unix_secs(),
            lease_token: claimed.lease_token.clone(),
            is_local_loopback: self.is_local_loopback,
        });
        let graph = build_action_graph(
            Arc::clone(&self.planner),
            self.retriever.clone(),
            context,
            checkpoint_store.clone(),
            effect_executor,
            self.memory.clone(),
        )
        .map_err(|e| PlannerUseCaseError::GraphRun(e.to_string()))?;
        let resumed_audit = serde_json::json!({
            "approval_source_event_id": resume_input
                .approval_source_event_id
                .as_ref()
                .map(crate::SourceEventId::as_str),
        })
        .to_string();
        self.store
            .append_audit(run_id, "resumed", &resumed_audit)
            .await?;
        let result = graph
            .resume(cid, resume_input)
            .await
            .map_err(|e| PlannerUseCaseError::GraphRun(e.to_string()))?;
        match result {
            agent_core::graph::GraphExecutionResult::Completed(completed) => {
                let draft = self.build_response_draft(completed.state.business());
                self.store
                    .mark_completed(run_id, &claimed.lease_token, draft.as_ref())
                    .await?;
                self.store.append_audit(run_id, "completed", "{}").await?;
                Ok(PlannerRunReport {
                    run_id: Some(run_id.as_str().to_owned()),
                    completed: true,
                    ..Default::default()
                })
            }
            agent_core::graph::GraphExecutionResult::Suspended(suspended) => {
                let next_checkpoint_id = suspended.checkpoint().id();
                let proposal_id = suspended.checkpoint().suspend().data.proposal_id.clone();
                let checkpoint_json = serde_json::json!({
                    "checkpoint_id": next_checkpoint_id.to_string(),
                    "proposal_id": proposal_id,
                    "reason": suspended.checkpoint().suspend().reason,
                    "next_node": suspended.checkpoint().position().next_node().as_str(),
                })
                .to_string();
                self.store
                    .mark_suspended(run_id, &claimed.lease_token, &checkpoint_json)
                    .await?;
                self.store
                    .append_audit(run_id, "suspended", &checkpoint_json)
                    .await?;
                Ok(PlannerRunReport {
                    run_id: Some(run_id.as_str().to_owned()),
                    suspended: true,
                    checkpoint_id: Some(next_checkpoint_id.to_string()),
                    proposal_id: Some(proposal_id),
                    ..Default::default()
                })
            }
        }
    }

    /// 处理运行失败：释放租约或标记退避。
    pub async fn handle_failure(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
        error: &PlannerUseCaseError,
        attempt: u32,
    ) -> Result<(), PlannerUseCaseError> {
        if error.is_retryable() {
            // 退避时间在 Rust 中饱和计算（约束 3）
            let backoff = backoff_ms(attempt, 500, 10_000);
            let next_eligible = self.clock.now_unix_secs() + (backoff / 1000) as i64;
            self.store
                .mark_failed(run_id, lease_token, &error.to_string(), next_eligible)
                .await?;
        } else {
            // 不可重试，释放租约让管理员介入
            warn!(
                run_id = run_id.as_str(),
                error = %error,
                "action run failed permanently, releasing lease"
            );
            self.store.release_lease(run_id, lease_token).await?;
        }
        Ok(())
    }

    pub fn lease_secs(&self) -> u64 {
        self.lease_secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActionLeaseToken, ActionRunId, ActionRunSeed, ActionStoreError, ActionStoreT,
        ClaimedActionRun, MessageSource, OwnerResponseDraft, PlannerError, PlannerInput,
        PlannerOutput, RecentEventRef, SecretaryActionEffect, SecretaryActionReceipt,
        SecretaryAgentState, SourceAccountRef, SourceEventId,
    };
    use agent_core::graph::InMemoryCheckpointStore;
    use async_trait::async_trait;
    use std::sync::Mutex;

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    struct FakePlanner {
        output: PlannerOutput,
    }

    #[async_trait]
    impl crate::ActionPlannerT for FakePlanner {
        async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
            Ok(self.output.clone())
        }
    }

    #[derive(Default)]
    struct FakeStore {
        completed: Mutex<Vec<String>>,
        failed: Mutex<Vec<(String, i64)>>,
        released: Mutex<Vec<String>>,
        claimed: Mutex<bool>,
    }

    #[async_trait]
    impl ActionStoreT for FakeStore {
        async fn ensure_action_run(
            &self,
            _run_id: &ActionRunId,
            _seed: &ActionRunSeed,
        ) -> Result<bool, ActionStoreError> {
            Ok(true)
        }
        async fn claim_pending_run(
            &self,
            _worker_id: &str,
            _lease_secs: u64,
            _now_unix_secs: i64,
        ) -> Result<Option<ClaimedActionRun>, ActionStoreError> {
            let mut claimed = self.claimed.lock().unwrap();
            if *claimed {
                return Ok(None);
            }
            *claimed = true;
            Ok(Some(ClaimedActionRun {
                run_id: ActionRunId::new("run-1").unwrap(),
                lease_token: ActionLeaseToken::generate(),
                account: account(),
                command_source_event_id: SourceEventId::new("event-1").unwrap(),
                command_text: "查最近消息".into(),
                conversation_id: "conv-1".into(),
                occurred_at_unix_secs: 1000,
                timezone_offset_secs: 28_800,
                timezone: "Asia/Shanghai".into(),
                recent_events: vec![RecentEventRef {
                    source_event_id: SourceEventId::new("event-1").unwrap(),
                    summary: "Owner 命令".into(),
                }],
            }))
        }
        async fn claim_suspended_run(
            &self,
            _claim: &SuspendedRunClaim,
        ) -> Result<Option<ClaimedActionRun>, ActionStoreError> {
            Ok(None)
        }
        async fn list_suspended_runs(
            &self,
            _account: &SourceAccountRef,
            _limit: u32,
        ) -> Result<Vec<crate::SuspendedActionRun>, ActionStoreError> {
            Ok(Vec::new())
        }
        async fn mark_suspended(
            &self,
            _run_id: &ActionRunId,
            _lease_token: &ActionLeaseToken,
            _checkpoint_json: &str,
        ) -> Result<(), ActionStoreError> {
            Ok(())
        }
        async fn load_checkpoint(
            &self,
            _run_id: &ActionRunId,
        ) -> Result<Option<String>, ActionStoreError> {
            Ok(None)
        }
        async fn take_checkpoint(
            &self,
            _run_id: &ActionRunId,
            _lease_token: &ActionLeaseToken,
        ) -> Result<Option<String>, ActionStoreError> {
            Ok(None)
        }
        async fn load_effect_receipt(
            &self,
            _run_id: &ActionRunId,
            _effect_id: &str,
        ) -> Result<Option<SecretaryActionReceipt>, ActionStoreError> {
            Ok(None)
        }
        async fn apply_effect(
            &self,
            _run_id: &ActionRunId,
            effect: &SecretaryActionEffect,
            _effect_id: &str,
            result_ref: &str,
            _lease_token: &ActionLeaseToken,
        ) -> Result<SecretaryActionReceipt, ActionStoreError> {
            Ok(SecretaryActionReceipt {
                proposal_id: effect.proposal.proposal_id.clone(),
                result_ref: result_ref.into(),
                tool_kind: None,
            })
        }
        async fn mark_completed(
            &self,
            run_id: &ActionRunId,
            _lease_token: &ActionLeaseToken,
            _response_draft: Option<&OwnerResponseDraft>,
        ) -> Result<(), ActionStoreError> {
            self.completed.lock().unwrap().push(run_id.as_str().into());
            Ok(())
        }
        async fn mark_failed(
            &self,
            run_id: &ActionRunId,
            _lease_token: &ActionLeaseToken,
            _error: &str,
            next_eligible_at_unix_secs: i64,
        ) -> Result<(), ActionStoreError> {
            self.failed
                .lock()
                .unwrap()
                .push((run_id.as_str().into(), next_eligible_at_unix_secs));
            Ok(())
        }
        async fn release_lease(
            &self,
            run_id: &ActionRunId,
            _lease_token: &ActionLeaseToken,
        ) -> Result<(), ActionStoreError> {
            self.released.lock().unwrap().push(run_id.as_str().into());
            Ok(())
        }
        async fn append_audit(
            &self,
            _run_id: &ActionRunId,
            _event_kind: &str,
            _detail_json: &str,
        ) -> Result<(), ActionStoreError> {
            Ok(())
        }
    }

    fn use_case(planner_output: PlannerOutput) -> (PlannerUseCase, Arc<FakeStore>) {
        let store = Arc::new(FakeStore::default());
        let planner = Arc::new(FakePlanner {
            output: planner_output,
        });
        let checkpoint_store = Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new());
        let use_case = PlannerUseCase::new(
            Arc::clone(&store) as Arc<dyn ActionStoreT>,
            planner,
            checkpoint_store,
            60,
        );
        (use_case, store)
    }

    #[tokio::test]
    async fn no_action_completes_without_effect() {
        let (use_case, store) = use_case(PlannerOutput::NoAction {
            reason: "无需处理".into(),
        });
        let report = use_case.run_once("worker-1").await.unwrap().unwrap();
        assert!(report.completed);
        assert!(!report.suspended);
        assert_eq!(store.completed.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn no_pending_run_returns_none() {
        let store = Arc::new(FakeStore::default());
        // 先 claim 一次耗尽
        let _ = store.claim_pending_run("", 60, 0).await.unwrap();
        let planner = Arc::new(FakePlanner {
            output: PlannerOutput::NoAction { reason: "x".into() },
        });
        let checkpoint = Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new());
        let use_case = PlannerUseCase::new(store, planner, checkpoint, 60);
        let result = use_case.run_once("worker-1").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn retryable_error_is_retryable() {
        let error = PlannerUseCaseError::Store(ActionStoreError::Database("conn lost".into()));
        assert!(error.is_retryable());
    }

    #[tokio::test]
    async fn lease_lost_not_retryable() {
        let error = PlannerUseCaseError::Store(ActionStoreError::LeaseLost);
        assert!(!error.is_retryable());
    }

    #[tokio::test]
    async fn unknown_commit_not_retryable() {
        let error = PlannerUseCaseError::Store(ActionStoreError::UnknownCommit("maybe".into()));
        assert!(!error.is_retryable());
    }

    #[tokio::test]
    async fn timeout_is_retryable() {
        let error = PlannerUseCaseError::Timeout;
        assert!(error.is_retryable());
    }

    #[tokio::test]
    async fn invalid_data_not_retryable() {
        let error = PlannerUseCaseError::Store(ActionStoreError::InvalidData("bad".into()));
        assert!(!error.is_retryable());
    }
}
