//! Action Graph 领域节点、EffectExecutor 与 Store 端口。
//!
//! Graph 拓扑（约束 4：Effect 只能通过 EffectExecutor 执行一次）：
//! ```text
//! Plan -> Gate(内联) -> L0Execute -> ReplanDecision -> (continue: Plan) | (finish: BuildResponse) -> End
//! Plan -> Gate -> Suspend(Approval) -> [resume] -> L0Execute -> BuildResponse -> End
//! Plan -> Gate -> Suspend(ExternalInput) -> End
//! Plan -> NoAction -> End
//! ```
//!
//! Replan 循环：只读查询工具（SearchRecentEvents/ReadSourceEvent 等）执行后，
//! EffectExecutor 将结构化 JSON 结果写入 result_ref；ReplanDecisionNode 将其解析为
//! PlannerToolObservation 并追加到状态；ReplanRouter 判断预算是否耗尽，决定继续
//! Plan（让 LLM 看到观察）或进入 BuildResponse。
//!
//! 最大 Replan 轮数由 `MAX_REPLAN_ROUNDS` 控制；非查询工具或不可解析 result_ref
//! 直接进入 BuildResponse，不进入循环。
//!
//! 所有 Effect 只通过 `SecretaryActionEffectExecutor` 执行一次；EffectExecutor 内部
//! 调用 `ActionStoreT::apply_effect`，Store 用 effect_id 做幂等键。

mod effect_executor;
mod nodes;
mod port;

use std::sync::Arc;

use agent_core::graph::{NodeId, TransitionRule};

use crate::{SecretaryAgentState, SecretaryRiskLevel};

pub use effect_executor::SecretaryActionEffectExecutor;
pub use nodes::{
    ActionGraphError, BuildResponseNode, L0ExecuteNode, NoActionNode, PlanNode, ReplanDecisionNode,
    ReplanRouter,
};
pub use port::{
    ActionLeaseToken, ActionRunContext, ActionRunId, ActionRunSeed, ActionStoreError, ActionStoreT,
    ClaimedActionRun, SuspendedActionRun, SuspendedRunClaim,
};

/// 装配好的 Action Graph Runtime。
pub type ActionGraphRuntime = agent_core::graph::GraphRuntime<SecretaryAgentState>;

/// 装配 Action Graph。
///
/// 拓扑：`Plan -> L0Execute -> ReplanDecision -> (Plan | BuildResponse) -> End`
/// 以及 `Plan -> Suspend -> End`（挂起后由 Checkpoint 恢复）。
///
/// Replan 循环最多执行 MAX_REPLAN_ROUNDS 次查询工具；ReplanRouter 在
/// 预算耗尽或非查询 Action 后路由到 BuildResponse。
///
/// BuildResponse 逻辑由 Effect receipt 驱动，在应用层 run_once 中组装 OwnerResponseDraft。
pub fn build_action_graph(
    planner: Arc<dyn crate::ActionPlannerT>,
    retriever: Option<Arc<crate::RetrieverUseCase>>,
    context: Arc<ActionRunContext>,
    checkpoint_store: Arc<dyn agent_core::graph::CheckpointStore<SecretaryAgentState>>,
    effect_executor: Arc<SecretaryActionEffectExecutor>,
    // CMD-009 目标 C：冲突驱动回读依赖 MemoryUseCase 的 L0 证据读取。
    memory: Option<Arc<crate::MemoryUseCase>>,
) -> Result<ActionGraphRuntime, ActionGraphError> {
    use agent_core::graph::{GraphDefinition, GraphId, GraphPolicy};
    use std::collections::BTreeMap;
    use std::num::NonZeroU32;

    let mut graph = GraphDefinition::new(GraphId::try_from("secretary_action").unwrap());
    graph
        .add_node(Arc::new(PlanNode::new(
            planner,
            retriever,
            Arc::clone(&context),
        )?))
        .map_err(ActionGraphError::from_display)?;
    graph
        .add_node(Arc::new(L0ExecuteNode::new()?))
        .map_err(ActionGraphError::from_display)?;
    let mut replan_node = ReplanDecisionNode::new()?;
    if let Some(memory) = memory {
        replan_node = replan_node.with_conflict_re_read(
            memory,
            context.account.clone(),
            context.is_local_loopback,
        );
    }
    graph
        .add_node(Arc::new(replan_node))
        .map_err(ActionGraphError::from_display)?;
    graph
        .add_node(Arc::new(BuildResponseNode::new(Arc::clone(&context))?))
        .map_err(ActionGraphError::from_display)?;
    let plan = NodeId::try_from("plan").unwrap();
    let l0 = NodeId::try_from("l0_execute").unwrap();
    let replan = NodeId::try_from("replan_decision").unwrap();
    let build = NodeId::try_from("build_response").unwrap();

    graph.set_entry(plan.clone());
    graph
        .set_transition(plan.clone(), TransitionRule::Goto(l0.clone()))
        .unwrap();
    graph
        .set_transition(l0.clone(), TransitionRule::Goto(replan.clone()))
        .unwrap();
    // Replan 分支：continue → 回到 Plan 继续循环；finish → 进入 BuildResponse。
    let mut replan_targets = BTreeMap::new();
    replan_targets.insert(
        agent_core::graph::RouteKey::try_from("continue").unwrap(),
        plan.clone(),
    );
    replan_targets.insert(
        agent_core::graph::RouteKey::try_from("finish").unwrap(),
        build.clone(),
    );
    graph
        .set_transition(
            replan.clone(),
            TransitionRule::Branch {
                router: Arc::new(ReplanRouter),
                targets: replan_targets,
            },
        )
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
mod tests;
