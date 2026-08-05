//! Action Planner 装配。
//!
//! 必须在 QQ Open Platform 之前装配，因为 OwnerCommand 入站时需要 `PlannerUseCase`
//! 创建 `action_run`。LLM 禁用时降级为 [`NoopActionPlanner`]（总是返回 NoAction）。

use std::sync::Arc;

use personal_secretary::{
    ActionPlannerT, AgendaUseCase, CheckpointStore, ConservativeMemoryCandidateExtractor,
    FollowUpControlUseCase, InMemoryCheckpointStore, MemoryCandidateControlUseCase,
    MemoryCandidateUseCase, MemoryUseCase, NotificationPolicyUseCase, PlannerError, PlannerInput,
    PlannerOutput, PlannerUseCase, ResponseExpectationControlUseCase, RetrieverPolicy,
    RetrieverUseCase, SecretaryAgentState, SourceAccountRef, SystemClock, ThreadControlUseCase,
    ThreadLinkReviewUseCase,
};
use personal_secretary_mysql::{
    build_mysql_action_checkpoint_store_factory, build_mysql_action_store,
    build_mysql_agenda_store, build_mysql_follow_up_control_store,
    build_mysql_memory_candidate_control_store, build_mysql_memory_candidate_store,
    build_mysql_memory_store, build_mysql_notification_policy_store,
    build_mysql_response_expectation_control_store, build_mysql_retriever_store,
    build_mysql_thread_control_store, build_mysql_thread_link_store,
};
use sea_orm::DatabaseConnection;

use crate::action_planner_worker::spawn_action_planner_worker;
use crate::bootstrap::workers::WorkerHandles;
use crate::config::AppConfig;
use crate::llm::OpenAiCompatibleClient;
use crate::runtime::RuntimeError;

/// LLM 禁用时的保守 Action Planner：总是返回 NoAction，不执行任何动作。
struct NoopActionPlanner;

#[async_trait::async_trait]
impl ActionPlannerT for NoopActionPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::NoAction {
            reason: "LLM 已禁用，不执行动作规划".into(),
        })
    }
}

/// 装配 Action Planner。返回 `PlannerUseCase` 供 official_platform 注入；禁用时返回 `None`。
pub(crate) async fn assemble_action_planner(
    handles: &mut WorkerHandles,
    db: DatabaseConnection,
    config: &AppConfig,
    account: SourceAccountRef,
) -> Result<Option<Arc<PlannerUseCase>>, RuntimeError> {
    if !config.action_planner.enabled {
        tracing::info!("Action Planner 已禁用（action_planner.enabled=false）");
        return Ok(None);
    }
    let action_store = build_mysql_action_store(db.clone());
    let is_loopback = config.llm_endpoint_verified_loopback();
    let planner: Arc<dyn ActionPlannerT> = if config.llm.enabled {
        let client = Arc::new(
            OpenAiCompatibleClient::new(&config.llm)
                .map_err(|error| RuntimeError::Llm(error.to_string()))?,
        );
        Arc::new(
            crate::action_planner::LlmActionPlanner::from_openai(client)
                .map(|p| p.with_loopback(is_loopback))
                .map_err(|error| RuntimeError::Llm(error.to_string()))?,
        )
    } else {
        tracing::info!("LLM 已禁用；Action Planner 使用空 NoAction 规划器");
        Arc::new(NoopActionPlanner)
    };
    // P0-3 修复：注入 DatabaseConnection，per-run 构造绑定业务 ActionRunId 的 CheckpointStore。
    // P0-2 修复：接入 RetrieverUseCase，让 PlanNode 检索数据库证据 + EffectExecutor 执行真实查询。
    let retriever_store = build_mysql_retriever_store(db.clone());
    let retriever = Arc::new(RetrieverUseCase::new(
        retriever_store,
        RetrieverPolicy::default(),
    ));
    // 构造参数提供保守回退；生产路径用适配器工厂按业务 run_id 创建持久化 CheckpointStore。
    let placeholder_checkpoint: Arc<dyn CheckpointStore<SecretaryAgentState>> =
        Arc::new(InMemoryCheckpointStore::new());
    let agenda = Arc::new(AgendaUseCase::new(
        build_mysql_agenda_store(db.clone()),
        Arc::new(SystemClock),
    ));
    let notification_policy = Arc::new(NotificationPolicyUseCase::new(
        build_mysql_notification_policy_store(db.clone()),
        Arc::new(SystemClock),
    ));
    let memory = Arc::new(MemoryUseCase::new(build_mysql_memory_store(db.clone())));
    let thread_control = Arc::new(ThreadControlUseCase::new(build_mysql_thread_control_store(
        db.clone(),
    )));
    let follow_up_control = Arc::new(FollowUpControlUseCase::new(
        build_mysql_follow_up_control_store(db.clone()),
    ));
    let response_expectation_control = Arc::new(ResponseExpectationControlUseCase::new(
        build_mysql_response_expectation_control_store(db.clone()),
    ));
    // 记忆候选：提取 worker 在 thread_pipeline 按账号装配；此共享实例只服务
    // ListMemoryCandidates 只读查询，用保守提取器占位（其账户仅用于构造签名）。
    let memory_candidate = Arc::new(
        MemoryCandidateUseCase::new(
            build_mysql_memory_candidate_store(db.clone()),
            Arc::new(
                ConservativeMemoryCandidateExtractor::new(
                    config.memory_candidates.max_event_chars as usize,
                    config.memory_candidates.extractor_version.clone(),
                )
                .map_err(|error| RuntimeError::Config(error.to_string()))?,
            ),
            account,
            config.memory_candidates.max_events_per_batch,
            config.memory_candidates.max_event_chars,
            config.memory_candidates.max_total_input_chars,
            config.memory_candidates.lease_secs,
            config.llm_endpoint_verified_loopback(),
        )
        .map_err(|error| RuntimeError::Config(error.to_string()))?,
    );
    let memory_candidate_control = Arc::new(MemoryCandidateControlUseCase::new(
        build_mysql_memory_candidate_control_store(db.clone()),
    ));
    let thread_link_review = Arc::new(ThreadLinkReviewUseCase::new(build_mysql_thread_link_store(
        db.clone(),
    )));
    let use_case = Arc::new(
        PlannerUseCase::new(
            action_store,
            planner,
            placeholder_checkpoint,
            config.action_planner.lease_secs,
        )
        .with_retriever(retriever)
        .with_notification_policy(notification_policy)
        .with_agenda(agenda)
        .with_memory(memory)
        .with_thread_control(thread_control)
        .with_follow_up_control(follow_up_control)
        .with_response_expectation_control(response_expectation_control)
        .with_memory_candidate(memory_candidate)
        .with_memory_candidate_control(memory_candidate_control)
        .with_thread_link_review(thread_link_review)
        .with_loopback(is_loopback)
        .with_checkpoint_store_factory(build_mysql_action_checkpoint_store_factory(db)),
    );
    let handle = spawn_action_planner_worker(Arc::clone(&use_case), config.action_planner.clone());
    tracing::info!(
        lease_secs = config.action_planner.lease_secs,
        scan_interval_ms = config.action_planner.scan_interval_ms,
        "Action Planner Worker 已装配"
    );
    handles.action_planner = Some(handle);
    Ok(Some(use_case))
}
