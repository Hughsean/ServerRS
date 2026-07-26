//! Action Graph 领域节点、EffectExecutor 与 Store 端口。
//!
//! Graph 拓扑（约束 4：Effect 只能通过 EffectExecutor 执行一次）：
//! ```text
//! Plan -> Gate(内联) -> L0Execute -> BuildResponse -> End
//! Plan -> Gate -> Suspend(Approval) -> [resume] -> L0Execute -> BuildResponse -> End
//! Plan -> Gate -> Suspend(ExternalInput) -> End
//! Plan -> NoAction -> End
//! ```
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
pub use nodes::{ActionGraphError, BuildResponseNode, L0ExecuteNode, NoActionNode, PlanNode};
pub use port::{
    ActionLeaseToken, ActionRunContext, ActionRunId, ActionRunSeed, ActionStoreError, ActionStoreT,
    ClaimedActionRun, SuspendedRunClaim,
};

/// 装配好的 Action Graph Runtime。
pub type ActionGraphRuntime = agent_core::graph::GraphRuntime<SecretaryAgentState>;

/// 装配 Action Graph。
///
/// 拓扑：`Plan -> (Gate 内联) -> L0Execute -> BuildResponse -> End`
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
    use agent_core::graph::{GraphDefinition, GraphId, GraphPolicy};
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
    graph
        .add_node(Arc::new(BuildResponseNode::new(Arc::clone(&context))?))
        .map_err(ActionGraphError::from_display)?;
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
mod tests;
