use super::{
    CompiledGraph, GraphRunError, NodeId, RunBudget, RunContext, RunId, RunTrace, TransitionRule,
    UsageSnapshot,
};
use crate::domain::agent::{AgentBusinessState, AgentState};
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::trace;

#[derive(Clone)]
pub struct GraphRuntime<B: AgentBusinessState> {
    graph: Arc<CompiledGraph<B>>,
}

impl<B: AgentBusinessState> GraphRuntime<B> {
    pub fn new(graph: CompiledGraph<B>) -> Self {
        Self {
            graph: Arc::new(graph),
        }
    }

    pub fn graph(&self) -> &CompiledGraph<B> {
        &self.graph
    }

    pub async fn run(
        &self,
        state: AgentState<B>,
        budget: RunBudget,
    ) -> Result<GraphRunResult<B>, GraphRunError> {
        let context = RunContext::new(budget, CancellationToken::new(), RunTrace::default());
        self.run_with_context(state, context).await
    }

    /// 使用调用方提供的预算与取消上下文运行图。
    ///
    /// 取消或超时会丢弃正在执行的节点 Future 并停止后续节点。外部写入可能已经由
    /// 远端系统提交，因此这不等价于回滚；写节点仍需保守地处理未知提交状态。
    pub async fn run_with_context(
        &self,
        mut state: AgentState<B>,
        context: RunContext,
    ) -> Result<GraphRunResult<B>, GraphRunError> {
        let mut current = self.graph.entry().clone();
        let mut visited = Vec::new();

        loop {
            let run_step = context.check_ready(self.graph.policy().max_steps())?;
            let step = run_step.get();

            let node = self
                .graph
                .node(&current)
                .ok_or_else(|| GraphRunError::MissingNode {
                    node: current.clone(),
                })?;
            let started_at = Instant::now();
            trace!(
                graph_id = %self.graph.id(),
                run_id = %context.run_id(),
                node_id = %current,
                step,
                trace_id = context.trace().trace_id.as_deref().unwrap_or(""),
                "agent graph node started"
            );
            let cancellation = context.cancellation().clone();
            let deadline = tokio::time::Instant::from_std(context.deadline());
            let execution = node.execute(&state, &context);
            let node_result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(GraphRunError::Cancelled),
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(GraphRunError::DeadlineExceeded);
                }
                result = execution => result,
            };
            let result = match node_result {
                Ok(result) => result,
                Err(error) => return Err(error.into_graph_run(current.clone())),
            };
            context.budget().record_usage(result.usage)?;
            state.apply_updates(result.updates).map_err(|error| {
                GraphRunError::StateUpdateFailed {
                    node: current.clone(),
                    error,
                }
            })?;
            visited.push(current.clone());
            let usage = context.budget().snapshot();
            trace!(
                graph_id = %self.graph.id(),
                run_id = %context.run_id(),
                node_id = %current,
                step,
                elapsed_ms = started_at.elapsed().as_millis(),
                llm_calls = usage.llm_calls,
                tool_calls = usage.tool_calls,
                tokens = usage.tokens,
                "agent graph node completed"
            );

            let transition = self.graph.transition(&current).ok_or_else(|| {
                GraphRunError::MissingTransition {
                    node: current.clone(),
                }
            })?;
            match transition {
                TransitionRule::Goto(next) => current = next.clone(),
                TransitionRule::Branch { router, targets } => {
                    let route =
                        router
                            .select(&state)
                            .map_err(|error| GraphRunError::NodeFailed {
                                node: current.clone(),
                                error,
                            })?;
                    trace!(
                        graph_id = %self.graph.id(),
                        run_id = %context.run_id(),
                        node_id = %current,
                        route = %route,
                        "agent graph route selected"
                    );
                    current = targets.get(&route).cloned().ok_or_else(|| {
                        GraphRunError::UnknownRoute {
                            node: current.clone(),
                            route,
                        }
                    })?;
                }
                TransitionRule::End => {
                    if state.outcome().is_none() {
                        return Err(GraphRunError::MissingOutcome);
                    }
                    return Ok(GraphRunResult {
                        state,
                        usage: context.budget().snapshot(),
                        visited,
                        run_id: context.run_id(),
                    });
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct GraphRunResult<B: AgentBusinessState> {
    pub state: AgentState<B>,
    pub usage: UsageSnapshot,
    pub visited: Vec<NodeId>,
    pub run_id: RunId,
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::domain::agent::{
        AgentBusinessState, AgentOutcome, AgentState, AgentStateError, AgentUpdate,
    };
    use crate::shared::error::AppError;
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    #[derive(Debug, Clone, Default)]
    struct TestBusiness {
        value: i32,
    }

    enum TestUpdate {
        Set(i32),
    }

    impl AgentBusinessState for TestBusiness {
        type Update = TestUpdate;
        type Effect = NoEffect<TestUpdate>;

        fn apply_update(&mut self, update: Self::Update) -> Result<(), AgentStateError> {
            match update {
                TestUpdate::Set(value) => self.value = value,
            }
            Ok(())
        }
    }

    enum Behavior {
        Noop,
        Set(i32),
        Finish,
        Fail,
        Usage(UsageDelta),
        ReserveLlm,
        Pending,
        ApplicationFailure(AppError),
    }

    struct FakeNode {
        id: NodeId,
        behavior: Behavior,
    }

    impl FakeNode {
        fn new(id: &str, behavior: Behavior) -> Self {
            Self {
                id: node_id(id),
                behavior,
            }
        }
    }

    #[async_trait]
    impl AgentNode<TestBusiness> for FakeNode {
        fn id(&self) -> &NodeId {
            &self.id
        }

        async fn execute(
            &self,
            _state: &AgentState<TestBusiness>,
            _context: &RunContext,
        ) -> Result<NodeResult<TestUpdate, NoEffect<TestUpdate>>, NodeError> {
            match self.behavior {
                Behavior::Noop => Ok(NodeResult::empty()),
                Behavior::Set(value) => Ok(NodeResult::new(
                    vec![AgentUpdate::Business(TestUpdate::Set(value))],
                    UsageDelta::default(),
                )),
                Behavior::Finish => Ok(NodeResult::new(
                    vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                        "done".into(),
                    ))],
                    UsageDelta::default(),
                )),
                Behavior::Fail => Err(NodeError::new(NodeErrorKind::Permanent, "failed")),
                Behavior::Usage(usage) => Ok(NodeResult::new(
                    vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                        "done".into(),
                    ))],
                    usage,
                )),
                Behavior::ReserveLlm => {
                    _context
                        .budget()
                        .reserve_llm_call()
                        .map_err(NodeError::from_graph_run)?;
                    Ok(NodeResult::new(
                        vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                            "done".into(),
                        ))],
                        UsageDelta::default(),
                    ))
                }
                Behavior::Pending => std::future::pending().await,
                Behavior::ApplicationFailure(ref error) => {
                    Err(NodeError::from_application(error.clone()))
                }
            }
        }
    }

    struct StaticRouter {
        known: Vec<RouteKey>,
        selected: RouteKey,
    }

    impl Router<TestBusiness> for StaticRouter {
        fn known_routes(&self) -> Vec<RouteKey> {
            self.known.clone()
        }

        fn select(&self, _state: &AgentState<TestBusiness>) -> Result<RouteKey, NodeError> {
            Ok(self.selected.clone())
        }
    }

    struct ValueRouter;

    impl Router<TestBusiness> for ValueRouter {
        fn known_routes(&self) -> Vec<RouteKey> {
            vec![route("zero"), route("nonzero")]
        }

        fn select(&self, state: &AgentState<TestBusiness>) -> Result<RouteKey, NodeError> {
            if state.business().value == 0 {
                Ok(route("zero"))
            } else {
                Ok(route("nonzero"))
            }
        }
    }

    fn node_id(value: &str) -> NodeId {
        NodeId::try_from(value).unwrap()
    }

    fn route(value: &str) -> RouteKey {
        RouteKey::try_from(value).unwrap()
    }

    fn policy(max_steps: u32) -> GraphPolicy {
        GraphPolicy::new(NonZeroU32::new(max_steps).unwrap())
    }

    fn direct_graph(start_behavior: Behavior) -> CompiledGraph<TestBusiness> {
        let mut graph = GraphDefinition::new(GraphId::try_from("direct").unwrap());
        graph
            .add_node(Arc::new(FakeNode::new("start", start_behavior)))
            .unwrap();
        graph
            .add_node(Arc::new(FakeNode::new("finish", Behavior::Finish)))
            .unwrap();
        graph.set_entry(node_id("start"));
        graph
            .set_transition(node_id("start"), TransitionRule::Goto(node_id("finish")))
            .unwrap();
        graph
            .set_transition(node_id("finish"), TransitionRule::End)
            .unwrap();
        graph.compile(policy(8)).unwrap()
    }

    fn single_node_graph(behavior: Behavior) -> CompiledGraph<TestBusiness> {
        let mut graph = GraphDefinition::new(GraphId::try_from("single").unwrap());
        graph
            .add_node(Arc::new(FakeNode::new("only", behavior)))
            .unwrap();
        graph.set_entry(node_id("only"));
        graph
            .set_transition(node_id("only"), TransitionRule::End)
            .unwrap();
        graph.compile(policy(8)).unwrap()
    }

    fn looping_graph(graph_max_steps: u32) -> CompiledGraph<TestBusiness> {
        let mut graph = GraphDefinition::new(GraphId::try_from("looping").unwrap());
        graph
            .add_node(Arc::new(FakeNode::new("loop", Behavior::Noop)))
            .unwrap();
        graph
            .add_node(Arc::new(FakeNode::new("finish", Behavior::Finish)))
            .unwrap();
        graph.set_entry(node_id("loop"));

        let router = StaticRouter {
            known: vec![route("again"), route("done")],
            selected: route("again"),
        };
        graph
            .set_transition(
                node_id("loop"),
                TransitionRule::Branch {
                    router: Arc::new(router),
                    targets: BTreeMap::from([
                        (route("again"), node_id("loop")),
                        (route("done"), node_id("finish")),
                    ]),
                },
            )
            .unwrap();
        graph
            .set_transition(node_id("finish"), TransitionRule::End)
            .unwrap();
        graph.compile(policy(graph_max_steps)).unwrap()
    }

    #[tokio::test]
    async fn router_observes_node_updates() {
        let mut graph = GraphDefinition::new(GraphId::try_from("branch").unwrap());
        graph
            .add_node(Arc::new(FakeNode::new("set", Behavior::Set(1))))
            .unwrap();
        graph
            .add_node(Arc::new(FakeNode::new("finish", Behavior::Finish)))
            .unwrap();
        graph
            .add_node(Arc::new(FakeNode::new("wrong", Behavior::Finish)))
            .unwrap();
        graph.set_entry(node_id("set"));
        graph
            .set_transition(
                node_id("set"),
                TransitionRule::Branch {
                    router: Arc::new(ValueRouter),
                    targets: BTreeMap::from([
                        (route("zero"), node_id("wrong")),
                        (route("nonzero"), node_id("finish")),
                    ]),
                },
            )
            .unwrap();
        graph
            .set_transition(node_id("finish"), TransitionRule::End)
            .unwrap();
        graph
            .set_transition(node_id("wrong"), TransitionRule::End)
            .unwrap();

        let result = GraphRuntime::new(graph.compile(policy(8)).unwrap())
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap();

        assert_eq!(result.state.business().value, 1);
        assert_eq!(result.visited, vec![node_id("set"), node_id("finish")]);
    }

    #[tokio::test]
    async fn follows_direct_transitions_and_returns_usage() {
        let result = GraphRuntime::new(direct_graph(Behavior::Noop))
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap();

        assert_eq!(result.visited, vec![node_id("start"), node_id("finish")]);
        assert_eq!(result.usage.steps, 2);
        assert_eq!(
            result.state.outcome().unwrap().response_text(),
            Some("done")
        );
    }

    #[tokio::test]
    async fn end_requires_an_outcome() {
        let error = GraphRuntime::new(single_node_graph(Behavior::Noop))
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, GraphRunError::MissingOutcome));
    }

    #[tokio::test]
    async fn rejects_router_keys_not_declared_at_compile_time() {
        let mut graph = GraphDefinition::new(GraphId::try_from("unknown-route").unwrap());
        graph
            .add_node(Arc::new(FakeNode::new("branch", Behavior::Noop)))
            .unwrap();
        graph
            .add_node(Arc::new(FakeNode::new("finish", Behavior::Finish)))
            .unwrap();
        graph.set_entry(node_id("branch"));
        graph
            .set_transition(
                node_id("branch"),
                TransitionRule::Branch {
                    router: Arc::new(StaticRouter {
                        known: vec![route("declared")],
                        selected: route("unexpected"),
                    }),
                    targets: BTreeMap::from([(route("declared"), node_id("finish"))]),
                },
            )
            .unwrap();
        graph
            .set_transition(node_id("finish"), TransitionRule::End)
            .unwrap();

        let error = GraphRuntime::new(graph.compile(policy(8)).unwrap())
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, GraphRunError::UnknownRoute { .. }));
    }

    #[tokio::test]
    async fn preserves_node_error_classification() {
        let error = GraphRuntime::new(single_node_graph(Behavior::Fail))
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        match error {
            GraphRunError::NodeFailed { error, .. } => {
                assert_eq!(error.kind(), NodeErrorKind::Permanent)
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn preserves_application_error_through_runtime() {
        let error = GraphRuntime::new(single_node_graph(Behavior::ApplicationFailure(
            AppError::Conflict("turn changed".into()),
        )))
        .run(
            AgentState::new(TestBusiness::default()),
            RunBudget::for_test(8),
        )
        .await
        .unwrap_err();

        match error {
            GraphRunError::NodeFailed { error, .. } => assert!(matches!(
                error.application_error(),
                Some(AppError::Conflict(message)) if message == "turn changed"
            )),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_budget_limits_steps() {
        let error = GraphRuntime::new(looping_graph(8))
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(2),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            GraphRunError::BudgetExceeded {
                resource: BudgetResource::Steps,
                limit: 2,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn compiled_graph_policy_also_limits_steps() {
        let error = GraphRuntime::new(looping_graph(1))
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            GraphRunError::BudgetExceeded {
                resource: BudgetResource::Steps,
                limit: 1,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn expired_deadline_stops_before_node_execution() {
        let budget = RunBudget::new(NonZeroU32::new(8).unwrap(), Duration::ZERO);
        let error = GraphRuntime::new(single_node_graph(Behavior::Finish))
            .run(AgentState::new(TestBusiness::default()), budget)
            .await
            .unwrap_err();

        assert!(matches!(error, GraphRunError::DeadlineExceeded));
    }

    #[tokio::test]
    async fn pre_cancelled_run_stops_before_node_execution() {
        let token = CancellationToken::new();
        token.cancel();
        let context = RunContext::new(RunBudget::for_test(8), token, RunTrace::default());

        let error = GraphRuntime::new(single_node_graph(Behavior::Finish))
            .run_with_context(AgentState::new(TestBusiness::default()), context)
            .await
            .unwrap_err();

        assert!(matches!(error, GraphRunError::Cancelled));
    }

    #[tokio::test]
    async fn deadline_interrupts_a_node_that_never_completes() {
        let budget = RunBudget::new(NonZeroU32::new(8).unwrap(), Duration::from_millis(10));

        let runtime = GraphRuntime::new(single_node_graph(Behavior::Pending));
        let run = runtime.run(AgentState::new(TestBusiness::default()), budget);
        let error = tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .expect("runtime did not enforce its deadline")
            .unwrap_err();

        assert!(matches!(error, GraphRunError::DeadlineExceeded));
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_node_that_is_already_running() {
        let token = CancellationToken::new();
        let cancel = token.clone();
        let context = RunContext::new(RunBudget::for_test(8), token, RunTrace::default());
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel.cancel();
        });

        let runtime = GraphRuntime::new(single_node_graph(Behavior::Pending));
        let run = runtime.run_with_context(AgentState::new(TestBusiness::default()), context);
        let error = tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .expect("runtime did not observe cancellation during node execution")
            .unwrap_err();

        assert!(matches!(error, GraphRunError::Cancelled));
    }

    #[tokio::test]
    async fn token_usage_limits_are_checked_before_state_updates() {
        let usage = UsageDelta { tokens: 11 };
        let budget = RunBudget::for_test(8).with_tokens(10);

        let error = GraphRuntime::new(single_node_graph(Behavior::Usage(usage)))
            .run(AgentState::new(TestBusiness::default()), budget)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            GraphRunError::BudgetExceeded {
                resource: BudgetResource::Tokens,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn node_budget_errors_remain_graph_budget_errors() {
        let budget = RunBudget::for_test(8).with_llm_calls(0);

        let error = GraphRuntime::new(single_node_graph(Behavior::ReserveLlm))
            .run(AgentState::new(TestBusiness::default()), budget)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            GraphRunError::BudgetExceeded {
                resource: BudgetResource::LlmCalls,
                ..
            }
        ));
    }

    #[test]
    fn budget_handle_reserves_calls_before_external_work() {
        let handle = RunBudgetHandle::new(
            RunBudget::for_test(8)
                .with_llm_calls(1)
                .with_tool_calls(2)
                .with_tokens(3),
        );

        handle.reserve_llm_call().unwrap();
        handle.reserve_tool_calls(2).unwrap();
        let reserved = handle.snapshot();
        assert_eq!(reserved.llm_calls, 1);
        assert_eq!(reserved.tool_calls, 2);
        handle.record_tokens(3).unwrap();

        assert!(matches!(
            handle.reserve_llm_call(),
            Err(GraphRunError::BudgetExceeded {
                resource: BudgetResource::LlmCalls,
                ..
            })
        ));
        assert!(matches!(
            handle.reserve_tool_calls(1),
            Err(GraphRunError::BudgetExceeded {
                resource: BudgetResource::ToolCalls,
                ..
            })
        ));
        assert!(matches!(
            handle.record_tokens(1),
            Err(GraphRunError::BudgetExceeded {
                resource: BudgetResource::Tokens,
                ..
            })
        ));
    }
}
