//! Action Planner 装配。
//!
//! 必须在 QQ Open Platform 之前装配，因为 OwnerCommand 入站时需要 `PlannerUseCase`
//! 创建 `action_run`。LLM 禁用时降级为 [`NoopActionPlanner`]（总是返回 NoAction）。

use std::sync::Arc;

use personal_secretary::{
    ActionPlannerT, CheckpointStore, InMemoryCheckpointStore, PlannerError, PlannerInput,
    PlannerOutput, PlannerUseCase, RetrieverPolicy, RetrieverUseCase, SecretaryAgentState,
    build_mysql_action_store, build_mysql_retriever_store,
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
) -> Result<Option<Arc<PlannerUseCase>>, RuntimeError> {
    if !config.action_planner.enabled {
        tracing::info!("Action Planner 已禁用（action_planner.enabled=false）");
        return Ok(None);
    }
    let action_store = build_mysql_action_store(db.clone());
    let planner: Arc<dyn ActionPlannerT> = if config.llm.enabled {
        let client = Arc::new(
            OpenAiCompatibleClient::new(&config.llm)
                .map_err(|error| RuntimeError::Llm(error.to_string()))?,
        );
        Arc::new(
            crate::action_planner::LlmActionPlanner::from_openai(client)
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
    // checkpoint_store 参数仅为满足签名；生产用 with_checkpoint_db 注入 MySQL。
    let placeholder_checkpoint: Arc<dyn CheckpointStore<SecretaryAgentState>> =
        Arc::new(InMemoryCheckpointStore::new());
    let use_case = Arc::new(
        PlannerUseCase::new(
            action_store,
            planner,
            placeholder_checkpoint,
            config.action_planner.lease_secs,
        )
        .with_retriever(retriever)
        .with_checkpoint_db(db),
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
