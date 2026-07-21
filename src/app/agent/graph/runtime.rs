use super::{
    AgentCheckpoint, AgentEffect, CheckpointError, CheckpointId, CheckpointRunError,
    CheckpointStore, CompiledGraph, EffectEnvelope, EffectError, EffectExecutor, EffectId,
    EffectReceipt, GraphExecutionResult, GraphRunError, NodeError, NodeErrorKind, NodeId,
    NodeResult, ResumeError, RunBudget, RunContext, RunId, RunPosition, RunTrace, SuspendedRun,
    TransitionRule, UsageSnapshot,
};
use crate::domain::agent::{AgentBusinessState, AgentState};
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::trace;

#[derive(Clone)]
pub struct GraphRuntime<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    graph: Arc<CompiledGraph<B>>,
    effect_executor: Option<Arc<dyn EffectExecutor<B::Effect>>>,
    checkpoint_store: Option<Arc<dyn CheckpointStore<B>>>,
}

impl<B> GraphRuntime<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    pub fn new(graph: CompiledGraph<B>) -> Self {
        Self {
            graph: Arc::new(graph),
            effect_executor: None,
            checkpoint_store: None,
        }
    }

    pub fn with_effect_executor(
        graph: CompiledGraph<B>,
        effect_executor: Arc<dyn EffectExecutor<B::Effect>>,
    ) -> Self {
        Self {
            graph: Arc::new(graph),
            effect_executor: Some(effect_executor),
            checkpoint_store: None,
        }
    }

    pub fn with_checkpoint_store(mut self, store: Arc<dyn CheckpointStore<B>>) -> Self {
        self.checkpoint_store = Some(store);
        self
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
        state: AgentState<B>,
        context: RunContext,
    ) -> Result<GraphRunResult<B>, GraphRunError> {
        match self
            .execute_from(
                state,
                context,
                self.graph.entry().clone(),
                Vec::new(),
                Vec::new(),
                false,
            )
            .await
        {
            Ok(GraphExecutionResult::Completed(result)) => Ok(result),
            Ok(GraphExecutionResult::Suspended(_)) => {
                unreachable!("completion-only execution cannot return Suspended")
            }
            Err(CheckpointRunError::Graph(error)) => Err(error),
            Err(CheckpointRunError::MissingStore) => {
                unreachable!("completion-only execution never requires a CheckpointStore")
            }
            Err(CheckpointRunError::SaveFailed { .. }) => {
                unreachable!("completion-only execution never saves a Checkpoint")
            }
        }
    }

    pub async fn run_checkpointed(
        &self,
        state: AgentState<B>,
        budget: RunBudget,
    ) -> Result<GraphExecutionResult<B>, CheckpointRunError<B>> {
        let context = RunContext::new(budget, CancellationToken::new(), RunTrace::default());
        self.run_checkpointed_with_context(state, context).await
    }

    pub async fn run_checkpointed_with_context(
        &self,
        state: AgentState<B>,
        context: RunContext,
    ) -> Result<GraphExecutionResult<B>, CheckpointRunError<B>> {
        if self.checkpoint_store.is_none() {
            return Err(CheckpointRunError::MissingStore);
        }
        self.execute_from(
            state,
            context,
            self.graph.entry().clone(),
            Vec::new(),
            Vec::new(),
            true,
        )
        .await
    }

    pub async fn resume(
        &self,
        checkpoint_id: CheckpointId,
        input: B::ResumeInput,
    ) -> Result<GraphExecutionResult<B>, ResumeError<B>> {
        self.resume_with_context(
            checkpoint_id,
            input,
            CancellationToken::new(),
            RunTrace::default(),
        )
        .await
    }

    pub async fn resume_with_context(
        &self,
        checkpoint_id: CheckpointId,
        input: B::ResumeInput,
        cancellation: CancellationToken,
        trace: RunTrace,
    ) -> Result<GraphExecutionResult<B>, ResumeError<B>> {
        let store = self
            .checkpoint_store
            .as_ref()
            .ok_or_else(|| ResumeError::RunFailed {
                source: Box::new(CheckpointRunError::MissingStore),
            })?;
        let checkpoint = store
            .load(checkpoint_id)
            .await
            .map_err(|source| ResumeError::CheckpointLoad { source })?;

        if checkpoint.graph_id() != self.graph.id() {
            return Err(ResumeError::GraphIdMismatch {
                expected: self.graph.id().clone(),
                actual: checkpoint.graph_id().clone(),
            });
        }
        if checkpoint.graph_version() != self.graph.version() {
            return Err(ResumeError::GraphVersionMismatch {
                expected: self.graph.version(),
                actual: checkpoint.graph_version(),
            });
        }
        let state_schema_version = B::state_schema_version();
        if checkpoint.state_schema_version() != state_schema_version {
            return Err(ResumeError::StateSchemaVersionMismatch {
                expected: state_schema_version,
                actual: checkpoint.state_schema_version(),
            });
        }
        let completed_step = checkpoint.position().completed_step().get();
        let usage_steps = checkpoint.usage().steps;
        if completed_step != usage_steps {
            return Err(ResumeError::RunPositionMismatch {
                completed_step,
                usage_steps,
            });
        }
        let next_node = checkpoint.position().next_node().clone();
        if self.graph.node(&next_node).is_none() {
            return Err(ResumeError::MissingNode { node: next_node });
        }

        let mut state = checkpoint.state().clone();
        state
            .apply_updates(B::resume_updates(input))
            .map_err(|error| ResumeError::ResumeInputRejected { error })?;

        store
            .take(checkpoint_id)
            .await
            .map_err(|error| match error {
                CheckpointError::NotFound { .. } => {
                    ResumeError::CheckpointAlreadyConsumed { checkpoint_id }
                }
                source => ResumeError::CheckpointLoad { source },
            })?;

        let mut restored_trace = checkpoint.trace().clone();
        if restored_trace.trace_id.is_none() {
            restored_trace.trace_id = trace.trace_id;
        }
        restored_trace.attributes.extend(trace.attributes);
        let context = RunContext::resume(
            checkpoint.run_id(),
            checkpoint.budget(),
            checkpoint.usage(),
            cancellation,
            restored_trace,
        );

        self.execute_from(
            state,
            context,
            next_node,
            checkpoint.visited().to_vec(),
            checkpoint.effect_receipts().to_vec(),
            true,
        )
        .await
        .map_err(|source| ResumeError::RunFailed {
            source: Box::new(source),
        })
    }

    async fn execute_from(
        &self,
        mut state: AgentState<B>,
        context: RunContext,
        mut current: NodeId,
        mut visited: Vec<NodeId>,
        mut effect_receipts: Vec<EffectReceipt<<B::Effect as AgentEffect>::Receipt>>,
        checkpointing: bool,
    ) -> Result<GraphExecutionResult<B>, CheckpointRunError<B>> {
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
                _ = cancellation.cancelled() => {
                    return Err(GraphRunError::Cancelled.into());
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(GraphRunError::DeadlineExceeded.into());
                }
                result = execution => result,
            };
            let result = match node_result {
                Ok(result) => result,
                Err(error) => return Err(error.into_graph_run(current.clone()).into()),
            };
            let (updates, effects, usage, suspend_request) = match result {
                NodeResult::Continue {
                    updates,
                    effects,
                    usage,
                } => (updates, effects, usage, None),
                NodeResult::Suspend {
                    updates,
                    effects,
                    usage,
                    request,
                } => {
                    if !checkpointing {
                        return Err(GraphRunError::UnexpectedSuspend {
                            node: current.clone(),
                        }
                        .into());
                    }
                    (updates, effects, usage, Some(request))
                }
            };
            let transition = self.graph.transition(&current).cloned().ok_or_else(|| {
                GraphRunError::MissingTransition {
                    node: current.clone(),
                }
            })?;
            if suspend_request.is_some() && matches!(transition, TransitionRule::End) {
                return Err(GraphRunError::SuspendAtEnd {
                    node: current.clone(),
                }
                .into());
            }
            context.budget().record_usage(usage)?;
            let mut candidate = state.clone();
            candidate
                .apply_updates(updates)
                .map_err(|error| GraphRunError::StateUpdateFailed {
                    node: current.clone(),
                    error,
                })?;

            let mut node_receipts = Vec::with_capacity(effects.len());
            let mut completed_effect_ids = Vec::with_capacity(effects.len());

            for (ordinal, effect) in effects.into_iter().enumerate() {
                let ordinal = u32::try_from(ordinal).map_err(|_| GraphRunError::NodeFailed {
                    node: current.clone(),
                    error: NodeError::new(
                        NodeErrorKind::Invariant,
                        "节点返回的 Effect 数量超过 u32",
                    ),
                })?;
                let effect_id = EffectId::new(context.run_id(), run_step, current.clone(), ordinal);
                let executor = self.effect_executor.as_ref().ok_or_else(|| {
                    GraphRunError::MissingEffectExecutor {
                        node: current.clone(),
                    }
                })?;
                context.check_active()?;
                let envelope = EffectEnvelope {
                    id: effect_id.clone(),
                    effect,
                };
                let cancellation = context.cancellation().clone();
                let deadline = tokio::time::Instant::from_std(context.deadline());
                let execution = executor.execute(&envelope, &context);
                let value = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        Err(EffectError::unknown_commit(
                            "Effect 执行期间运行被取消，外部提交状态未知",
                        ))
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        Err(EffectError::unknown_commit(
                            "Effect 执行期间超过截止时间，外部提交状态未知",
                        ))
                    }
                    result = execution => result,
                };

                let value = value.map_err(|error| GraphRunError::EffectFailed {
                    node: current.clone(),
                    effect_id: effect_id.clone(),
                    completed_effect_ids: completed_effect_ids.clone(),
                    error,
                })?;
                let receipt_updates = B::Effect::receipt_updates(&value);
                candidate.apply_updates(receipt_updates).map_err(|error| {
                    GraphRunError::PostEffectStateUpdateFailed {
                        node: current.clone(),
                        effect_id: effect_id.clone(),
                        error,
                    }
                })?;
                completed_effect_ids.push(effect_id.clone());
                node_receipts.push(EffectReceipt { effect_id, value });
            }

            state = candidate;
            effect_receipts.extend(node_receipts);
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

            let next_node = match transition {
                TransitionRule::Goto(next) => Some(next),
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
                    Some(targets.get(&route).cloned().ok_or_else(|| {
                        GraphRunError::UnknownRoute {
                            node: current.clone(),
                            route,
                        }
                    })?)
                }
                TransitionRule::End => {
                    if state.outcome().is_none() {
                        return Err(GraphRunError::MissingOutcome.into());
                    }
                    None
                }
            };

            if let Some(request) = suspend_request {
                let next_node = next_node.expect("SuspendAtEnd was rejected before execution");
                let checkpoint = AgentCheckpoint::new(
                    CheckpointId::new(),
                    self.graph.id().clone(),
                    self.graph.version(),
                    B::state_schema_version(),
                    context.run_id(),
                    RunPosition::new(run_step, next_node),
                    state,
                    context.budget().limits(),
                    context.budget().snapshot(),
                    visited,
                    effect_receipts,
                    request,
                    context.trace().clone(),
                );
                let checkpoint_id = checkpoint.id();
                let store = self
                    .checkpoint_store
                    .as_ref()
                    .ok_or(CheckpointRunError::MissingStore)?;
                if let Err(source) = store.save(checkpoint.clone()).await {
                    return Err(CheckpointRunError::SaveFailed {
                        checkpoint_id,
                        checkpoint: Box::new(checkpoint),
                        source,
                    });
                }
                return Ok(GraphExecutionResult::Suspended(SuspendedRun::new(
                    checkpoint,
                )));
            }

            match next_node {
                Some(next_node) => current = next_node,
                None => {
                    return Ok(GraphExecutionResult::Completed(GraphRunResult {
                        state,
                        usage: context.budget().snapshot(),
                        visited,
                        run_id: context.run_id(),
                        effect_receipts,
                    }));
                }
            }
        }
    }
}

pub struct GraphRunResult<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    pub state: AgentState<B>,
    pub usage: UsageSnapshot,
    pub visited: Vec<NodeId>,
    pub run_id: RunId,
    pub effect_receipts: Vec<EffectReceipt<<B::Effect as AgentEffect>::Receipt>>,
}

impl<B> std::fmt::Debug for GraphRunResult<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GraphRunResult")
            .field("usage", &self.usage)
            .field("visited", &self.visited)
            .field("run_id", &self.run_id)
            .field("effect_receipt_count", &self.effect_receipts.len())
            .finish_non_exhaustive()
    }
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    #[derive(Debug, Clone, Default)]
    struct TestBusiness {
        value: i32,
    }

    enum TestUpdate {
        Set(i32),
        Reject,
    }

    enum TestResumeInput {
        Set(i32),
        Reject,
    }

    #[derive(Debug, Clone)]
    enum TestEffect {
        Set(i32),
        Fail,
        RejectUpdate,
        Pending(Arc<tokio::sync::Notify>),
    }

    #[derive(Debug, Clone)]
    enum TestReceipt {
        Set(i32),
        Reject,
    }

    impl AgentEffect for TestEffect {
        type Update = TestUpdate;
        type Receipt = TestReceipt;

        fn receipt_updates(receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>> {
            match receipt {
                TestReceipt::Set(value) => {
                    vec![AgentUpdate::Business(TestUpdate::Set(*value))]
                }
                TestReceipt::Reject => vec![AgentUpdate::Business(TestUpdate::Reject)],
            }
        }
    }

    struct TestEffectExecutor {
        calls: AtomicUsize,
    }

    impl TestEffectExecutor {
        fn recording() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    struct FailingCheckpointStore {
        save_calls: AtomicUsize,
    }

    impl FailingCheckpointStore {
        fn new() -> Self {
            Self {
                save_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl CheckpointStore<TestBusiness> for FailingCheckpointStore {
        async fn save(
            &self,
            _checkpoint: AgentCheckpoint<TestBusiness>,
        ) -> Result<(), CheckpointError> {
            self.save_calls.fetch_add(1, Ordering::SeqCst);
            Err(CheckpointError::StoreUnavailable)
        }

        async fn load(
            &self,
            checkpoint_id: CheckpointId,
        ) -> Result<AgentCheckpoint<TestBusiness>, CheckpointError> {
            Err(CheckpointError::NotFound { checkpoint_id })
        }

        async fn take(
            &self,
            checkpoint_id: CheckpointId,
        ) -> Result<AgentCheckpoint<TestBusiness>, CheckpointError> {
            Err(CheckpointError::NotFound { checkpoint_id })
        }
    }

    #[async_trait]
    impl EffectExecutor<TestEffect> for TestEffectExecutor {
        async fn execute(
            &self,
            envelope: &EffectEnvelope<TestEffect>,
            _context: &RunContext,
        ) -> Result<TestReceipt, EffectError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &envelope.effect {
                TestEffect::Set(value) => Ok(TestReceipt::Set(*value)),
                TestEffect::Fail => Err(EffectError::from_application(AppError::Conflict(
                    "turn changed".into(),
                ))),
                TestEffect::RejectUpdate => Ok(TestReceipt::Reject),
                TestEffect::Pending(started) => {
                    started.notify_one();
                    std::future::pending::<Result<TestReceipt, EffectError>>().await
                }
            }
        }
    }

    impl AgentBusinessState for TestBusiness {
        type Update = TestUpdate;
        type Effect = TestEffect;
        type SuspendData = String;
        type ResumeInput = TestResumeInput;

        fn resume_updates(input: Self::ResumeInput) -> Vec<AgentUpdate<Self::Update>> {
            vec![AgentUpdate::Business(match input {
                TestResumeInput::Set(value) => TestUpdate::Set(value),
                TestResumeInput::Reject => TestUpdate::Reject,
            })]
        }

        fn apply_update(&mut self, update: Self::Update) -> Result<(), AgentStateError> {
            match update {
                TestUpdate::Set(value) => {
                    self.value = value;
                    Ok(())
                }
                TestUpdate::Reject => Err(AgentStateError::Business("rejected".into())),
            }
        }
    }

    enum Behavior {
        Noop,
        Set(i32),
        Finish,
        FinishFromState,
        Fail,
        Usage(UsageDelta),
        ReserveLlm,
        Effect(i32),
        Effects(Vec<TestEffect>),
        RejectThenEffect(TestEffect),
        CancelThenEffects(Vec<TestEffect>),
        UsageThenEffects(UsageDelta, Vec<TestEffect>),
        Suspend {
            update: i32,
            effect: i32,
            tokens: u64,
            data: String,
        },
        SuspendEffects {
            update: i32,
            effects: Vec<TestEffect>,
            data: String,
        },
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
            state: &AgentState<TestBusiness>,
            _context: &RunContext,
        ) -> Result<NodeResult<TestUpdate, TestEffect, String>, NodeError> {
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
                Behavior::FinishFromState => Ok(NodeResult::new(
                    vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                        state.business().value.to_string(),
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
                Behavior::Effect(value) => Ok(NodeResult::with_effect(
                    vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                        "done".into(),
                    ))],
                    TestEffect::Set(value),
                    UsageDelta::default(),
                )),
                Behavior::Effects(ref effects) => Ok(NodeResult::with_effects(
                    vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                        "done".into(),
                    ))],
                    effects.clone(),
                    UsageDelta::default(),
                )),
                Behavior::RejectThenEffect(ref effect) => Ok(NodeResult::with_effect(
                    vec![AgentUpdate::Business(TestUpdate::Reject)],
                    effect.clone(),
                    UsageDelta::default(),
                )),
                Behavior::CancelThenEffects(ref effects) => {
                    _context.cancellation().cancel();
                    Ok(NodeResult::with_effects(
                        vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                            "done".into(),
                        ))],
                        effects.clone(),
                        UsageDelta::default(),
                    ))
                }
                Behavior::UsageThenEffects(usage, ref effects) => Ok(NodeResult::with_effects(
                    vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                        "done".into(),
                    ))],
                    effects.clone(),
                    usage,
                )),
                Behavior::Suspend {
                    update,
                    effect,
                    tokens,
                    ref data,
                } => Ok(NodeResult::Suspend {
                    updates: vec![AgentUpdate::Business(TestUpdate::Set(update))],
                    effects: vec![TestEffect::Set(effect)],
                    usage: UsageDelta { tokens },
                    request: SuspendRequest::new(SuspendReason::ExternalInput, data.clone()),
                }),
                Behavior::SuspendEffects {
                    update,
                    ref effects,
                    ref data,
                } => Ok(NodeResult::Suspend {
                    updates: vec![AgentUpdate::Business(TestUpdate::Set(update))],
                    effects: effects.clone(),
                    usage: UsageDelta::default(),
                    request: SuspendRequest::new(SuspendReason::ExternalInput, data.clone()),
                }),
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
        direct_graph_with_finish(start_behavior, Behavior::Finish, GraphVersion::initial())
    }

    fn direct_graph_versioned(
        start_behavior: Behavior,
        version: GraphVersion,
    ) -> CompiledGraph<TestBusiness> {
        direct_graph_with_finish(start_behavior, Behavior::Finish, version)
    }

    fn direct_graph_with_finish(
        start_behavior: Behavior,
        finish_behavior: Behavior,
        version: GraphVersion,
    ) -> CompiledGraph<TestBusiness> {
        let mut graph =
            GraphDefinition::new_versioned(GraphId::try_from("direct").unwrap(), version);
        graph
            .add_node(Arc::new(FakeNode::new("start", start_behavior)))
            .unwrap();
        graph
            .add_node(Arc::new(FakeNode::new("finish", finish_behavior)))
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

    fn manual_checkpoint(
        graph_id: GraphId,
        graph_version: GraphVersion,
        state_schema_version: StateSchemaVersion,
        next_node: NodeId,
        completed_step: u32,
        usage_steps: u32,
    ) -> AgentCheckpoint<TestBusiness> {
        AgentCheckpoint::new(
            CheckpointId::new(),
            graph_id,
            graph_version,
            state_schema_version,
            RunId::new(),
            RunPosition::new(RunStep::try_from(completed_step).unwrap(), next_node),
            AgentState::new(TestBusiness::default()),
            RunBudget::for_test(8),
            UsageSnapshot {
                steps: usage_steps,
                llm_calls: 0,
                tool_calls: 0,
                tokens: 0,
            },
            vec![node_id("start")],
            Vec::new(),
            SuspendRequest::new(SuspendReason::ExternalInput, "ticket".into()),
            RunTrace::default(),
        )
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
    async fn checkpointed_run_resumes_from_next_node_with_same_run_and_receipts() {
        let store = Arc::new(InMemoryCheckpointStore::<TestBusiness>::new());
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            direct_graph_with_finish(
                Behavior::Suspend {
                    update: 5,
                    effect: 7,
                    tokens: 3,
                    data: "ticket-42".into(),
                },
                Behavior::FinishFromState,
                GraphVersion::initial(),
            ),
            executor.clone(),
        )
        .with_checkpoint_store(store.clone());

        let suspended = match runtime
            .run_checkpointed(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap()
        {
            GraphExecutionResult::Suspended(suspended) => suspended,
            GraphExecutionResult::Completed(_) => panic!("expected suspension"),
        };
        let checkpoint_id = suspended.checkpoint().id();
        let run_id = suspended.checkpoint().run_id();

        assert_eq!(
            suspended.checkpoint().position().next_node(),
            &node_id("finish")
        );
        assert_eq!(suspended.checkpoint().usage().steps, 1);
        assert_eq!(suspended.checkpoint().usage().tokens, 3);
        assert_eq!(suspended.checkpoint().state().business().value, 7);
        assert_eq!(suspended.checkpoint().effect_receipts().len(), 1);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);

        let completed = match runtime
            .resume(checkpoint_id, TestResumeInput::Set(11))
            .await
            .unwrap()
        {
            GraphExecutionResult::Completed(completed) => completed,
            GraphExecutionResult::Suspended(_) => panic!("expected completion"),
        };

        assert_eq!(completed.run_id, run_id);
        assert_eq!(completed.usage.steps, 2);
        assert_eq!(completed.usage.tokens, 3);
        assert_eq!(completed.visited, vec![node_id("start"), node_id("finish")]);
        assert_eq!(completed.state.business().value, 11);
        assert_eq!(
            completed
                .state
                .outcome()
                .and_then(AgentOutcome::response_text),
            Some("11")
        );
        assert_eq!(completed.effect_receipts.len(), 1);
        assert_eq!(completed.effect_receipts[0].effect_id.step().get(), 1);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            store.load(checkpoint_id).await,
            Err(CheckpointError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn resume_rejects_graph_version_mismatch_without_consuming_checkpoint() {
        let store = Arc::new(InMemoryCheckpointStore::<TestBusiness>::new());
        let runtime = GraphRuntime::with_effect_executor(
            direct_graph(Behavior::Suspend {
                update: 5,
                effect: 7,
                tokens: 0,
                data: "ticket".into(),
            }),
            Arc::new(TestEffectExecutor::recording()),
        )
        .with_checkpoint_store(store.clone());
        let checkpoint_id = match runtime
            .run_checkpointed(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap()
        {
            GraphExecutionResult::Suspended(suspended) => suspended.checkpoint().id(),
            GraphExecutionResult::Completed(_) => panic!("expected suspension"),
        };

        let version_two = GraphVersion::try_from(2).unwrap();
        let incompatible = GraphRuntime::new(direct_graph_versioned(Behavior::Noop, version_two))
            .with_checkpoint_store(store.clone());
        let error = incompatible
            .resume(checkpoint_id, TestResumeInput::Set(11))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ResumeError::GraphVersionMismatch { expected, actual }
                if expected == version_two && actual == GraphVersion::initial()
        ));
        assert!(store.load(checkpoint_id).await.is_ok());
    }

    #[tokio::test]
    async fn resume_rejects_graph_id_mismatch_without_consuming_checkpoint() {
        let store = Arc::new(InMemoryCheckpointStore::<TestBusiness>::new());
        let checkpoint = manual_checkpoint(
            GraphId::try_from("other").unwrap(),
            GraphVersion::initial(),
            StateSchemaVersion::initial(),
            node_id("finish"),
            1,
            1,
        );
        let checkpoint_id = checkpoint.id();
        store.save(checkpoint).await.unwrap();
        let runtime =
            GraphRuntime::new(direct_graph(Behavior::Noop)).with_checkpoint_store(store.clone());

        let error = runtime
            .resume(checkpoint_id, TestResumeInput::Set(11))
            .await
            .unwrap_err();

        assert!(matches!(error, ResumeError::GraphIdMismatch { .. }));
        assert!(store.load(checkpoint_id).await.is_ok());
    }

    #[tokio::test]
    async fn resume_rejects_state_schema_mismatch_without_consuming_checkpoint() {
        let store = Arc::new(InMemoryCheckpointStore::<TestBusiness>::new());
        let checkpoint = manual_checkpoint(
            GraphId::try_from("direct").unwrap(),
            GraphVersion::initial(),
            StateSchemaVersion::try_from(2).unwrap(),
            node_id("finish"),
            1,
            1,
        );
        let checkpoint_id = checkpoint.id();
        store.save(checkpoint).await.unwrap();
        let runtime =
            GraphRuntime::new(direct_graph(Behavior::Noop)).with_checkpoint_store(store.clone());

        let error = runtime
            .resume(checkpoint_id, TestResumeInput::Set(11))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ResumeError::StateSchemaVersionMismatch { expected, actual }
                if expected == StateSchemaVersion::initial() && actual.get() == 2
        ));
        assert!(store.load(checkpoint_id).await.is_ok());
    }

    #[tokio::test]
    async fn resume_rejects_missing_next_node_without_consuming_checkpoint() {
        let store = Arc::new(InMemoryCheckpointStore::<TestBusiness>::new());
        let checkpoint = manual_checkpoint(
            GraphId::try_from("direct").unwrap(),
            GraphVersion::initial(),
            StateSchemaVersion::initial(),
            node_id("missing"),
            1,
            1,
        );
        let checkpoint_id = checkpoint.id();
        store.save(checkpoint).await.unwrap();
        let runtime =
            GraphRuntime::new(direct_graph(Behavior::Noop)).with_checkpoint_store(store.clone());

        let error = runtime
            .resume(checkpoint_id, TestResumeInput::Set(11))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ResumeError::MissingNode { node } if node == node_id("missing")
        ));
        assert!(store.load(checkpoint_id).await.is_ok());
    }

    #[tokio::test]
    async fn resume_rejects_run_position_mismatch_without_consuming_checkpoint() {
        let store = Arc::new(InMemoryCheckpointStore::<TestBusiness>::new());
        let checkpoint = manual_checkpoint(
            GraphId::try_from("direct").unwrap(),
            GraphVersion::initial(),
            StateSchemaVersion::initial(),
            node_id("finish"),
            2,
            1,
        );
        let checkpoint_id = checkpoint.id();
        store.save(checkpoint).await.unwrap();
        let runtime =
            GraphRuntime::new(direct_graph(Behavior::Noop)).with_checkpoint_store(store.clone());

        let error = runtime
            .resume(checkpoint_id, TestResumeInput::Set(11))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ResumeError::RunPositionMismatch {
                completed_step: 2,
                usage_steps: 1,
            }
        ));
        assert!(store.load(checkpoint_id).await.is_ok());
    }

    #[tokio::test]
    async fn rejected_resume_input_keeps_checkpoint_and_does_not_run_next_node() {
        let store = Arc::new(InMemoryCheckpointStore::<TestBusiness>::new());
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            direct_graph(Behavior::Suspend {
                update: 5,
                effect: 7,
                tokens: 0,
                data: "ticket".into(),
            }),
            executor.clone(),
        )
        .with_checkpoint_store(store.clone());
        let checkpoint_id = match runtime
            .run_checkpointed(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap()
        {
            GraphExecutionResult::Suspended(suspended) => suspended.checkpoint().id(),
            GraphExecutionResult::Completed(_) => panic!("expected suspension"),
        };

        let error = runtime
            .resume(checkpoint_id, TestResumeInput::Reject)
            .await
            .unwrap_err();

        assert!(matches!(error, ResumeError::ResumeInputRejected { .. }));
        assert!(store.load(checkpoint_id).await.is_ok());
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn completion_only_entry_rejects_suspend_before_effect_dispatch() {
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            direct_graph(Behavior::Suspend {
                update: 5,
                effect: 7,
                tokens: 0,
                data: "ticket".into(),
            }),
            executor.clone(),
        );

        let error = runtime
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, GraphRunError::UnexpectedSuspend { .. }));
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn checkpointed_run_requires_store_before_node_execution() {
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            direct_graph(Behavior::Suspend {
                update: 5,
                effect: 7,
                tokens: 0,
                data: "ticket".into(),
            }),
            executor.clone(),
        );

        let error = runtime
            .run_checkpointed(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, CheckpointRunError::MissingStore));
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn suspend_at_end_is_rejected_before_effect_dispatch() {
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            single_node_graph(Behavior::Suspend {
                update: 5,
                effect: 7,
                tokens: 0,
                data: "ticket".into(),
            }),
            executor.clone(),
        )
        .with_checkpoint_store(Arc::new(InMemoryCheckpointStore::new()));

        let error = runtime
            .run_checkpointed(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CheckpointRunError::Graph(GraphRunError::SuspendAtEnd { .. })
        ));
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn checkpoint_save_failure_returns_unsaved_state_without_replaying_effect() {
        let executor = Arc::new(TestEffectExecutor::recording());
        let store = Arc::new(FailingCheckpointStore::new());
        let runtime = GraphRuntime::with_effect_executor(
            direct_graph(Behavior::Suspend {
                update: 5,
                effect: 7,
                tokens: 0,
                data: "ticket".into(),
            }),
            executor.clone(),
        )
        .with_checkpoint_store(store.clone());

        let error = runtime
            .run_checkpointed(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        match error {
            CheckpointRunError::SaveFailed {
                checkpoint,
                source: CheckpointError::StoreUnavailable,
                ..
            } => {
                assert_eq!(checkpoint.state().business().value, 7);
                assert_eq!(checkpoint.effect_receipts().len(), 1);
                assert_eq!(checkpoint.position().next_node(), &node_id("finish"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.save_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn effect_ids_continue_across_resume_without_collision() {
        let store = Arc::new(InMemoryCheckpointStore::<TestBusiness>::new());
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            direct_graph_with_finish(
                Behavior::Suspend {
                    update: 5,
                    effect: 7,
                    tokens: 0,
                    data: "ticket".into(),
                },
                Behavior::Effect(9),
                GraphVersion::initial(),
            ),
            executor.clone(),
        )
        .with_checkpoint_store(store);
        let suspended = match runtime
            .run_checkpointed(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap()
        {
            GraphExecutionResult::Suspended(suspended) => suspended,
            GraphExecutionResult::Completed(_) => panic!("expected suspension"),
        };
        let checkpoint_id = suspended.checkpoint().id();
        let first_id = suspended.checkpoint().effect_receipts()[0]
            .effect_id
            .clone();

        let completed = match runtime
            .resume(checkpoint_id, TestResumeInput::Set(11))
            .await
            .unwrap()
        {
            GraphExecutionResult::Completed(completed) => completed,
            GraphExecutionResult::Suspended(_) => panic!("expected completion"),
        };

        let second_id = &completed.effect_receipts[1].effect_id;
        assert_eq!(completed.effect_receipts.len(), 2);
        assert_eq!(first_id.run_id(), second_id.run_id());
        assert_eq!(first_id.step().get(), 1);
        assert_eq!(second_id.step().get(), 2);
        assert_ne!(first_id, *second_id);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn unknown_commit_does_not_create_a_checkpoint() {
        let token = CancellationToken::new();
        let cancel = token.clone();
        let started = Arc::new(tokio::sync::Notify::new());
        let context = RunContext::new(RunBudget::for_test(8), token, RunTrace::default());
        let executor = Arc::new(TestEffectExecutor::recording());
        let store = Arc::new(InMemoryCheckpointStore::<TestBusiness>::new());
        let runtime = GraphRuntime::with_effect_executor(
            direct_graph(Behavior::SuspendEffects {
                update: 5,
                effects: vec![TestEffect::Pending(started.clone())],
                data: "ticket".into(),
            }),
            executor,
        )
        .with_checkpoint_store(store.clone());

        let mut run = tokio::spawn(async move {
            runtime
                .run_checkpointed_with_context(AgentState::new(TestBusiness::default()), context)
                .await
        });
        started.notified().await;
        cancel.cancel();

        let joined = match tokio::time::timeout(Duration::from_secs(1), &mut run).await {
            Ok(joined) => joined,
            Err(_) => {
                run.abort();
                panic!("runtime did not observe in-flight cancellation");
            }
        };
        let error = joined.unwrap().unwrap_err();

        match error {
            CheckpointRunError::Graph(GraphRunError::EffectFailed { error, .. }) => {
                assert_eq!(error.kind(), EffectErrorKind::UnknownCommit)
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(store.is_empty().unwrap());
    }

    #[tokio::test]
    async fn resume_keeps_the_original_step_budget_instead_of_resetting_it() {
        let store = Arc::new(InMemoryCheckpointStore::<TestBusiness>::new());
        let runtime = GraphRuntime::with_effect_executor(
            direct_graph(Behavior::Suspend {
                update: 5,
                effect: 7,
                tokens: 0,
                data: "ticket".into(),
            }),
            Arc::new(TestEffectExecutor::recording()),
        )
        .with_checkpoint_store(store.clone());
        let checkpoint_id = match runtime
            .run_checkpointed(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(1),
            )
            .await
            .unwrap()
        {
            GraphExecutionResult::Suspended(suspended) => suspended.checkpoint().id(),
            GraphExecutionResult::Completed(_) => panic!("expected suspension"),
        };

        let error = runtime
            .resume(checkpoint_id, TestResumeInput::Set(11))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ResumeError::RunFailed { source }
                if matches!(
                    *source,
                    CheckpointRunError::Graph(GraphRunError::BudgetExceeded {
                        resource: BudgetResource::Steps,
                        limit: 1,
                        attempted: 2,
                    })
                )
        ));
        assert!(matches!(
            store.load(checkpoint_id).await,
            Err(CheckpointError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn effect_receipt_updates_candidate_state_and_is_returned() {
        let executor = Arc::new(TestEffectExecutor {
            calls: AtomicUsize::new(0),
        });
        let runtime = GraphRuntime::with_effect_executor(
            single_node_graph(Behavior::Effect(7)),
            executor.clone(),
        );

        let result = runtime
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap();

        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.state.business().value, 7);
        assert_eq!(result.effect_receipts.len(), 1);
        assert_eq!(result.effect_receipts[0].effect_id.run_id(), result.run_id);
        assert_eq!(result.effect_receipts[0].effect_id.step().get(), 1);
        assert_eq!(
            result.effect_receipts[0].effect_id.node_id(),
            &node_id("only")
        );
        assert_eq!(result.effect_receipts[0].effect_id.ordinal(), 0);
    }

    #[tokio::test]
    async fn effect_without_executor_is_rejected() {
        let error = GraphRuntime::new(single_node_graph(Behavior::Effect(7)))
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            GraphRunError::MissingEffectExecutor { node } if node == node_id("only")
        ));
    }

    #[tokio::test]
    async fn failed_effect_is_not_retried_and_preserves_application_error() {
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            single_node_graph(Behavior::Effects(vec![TestEffect::Fail])),
            executor.clone(),
        );

        let error = runtime
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
        match error {
            GraphRunError::EffectFailed { error, .. } => assert!(matches!(
                error.application_error(),
                Some(AppError::Conflict(message)) if message == "turn changed"
            )),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn invalid_pure_update_prevents_effect_dispatch() {
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            single_node_graph(Behavior::RejectThenEffect(TestEffect::Set(7))),
            executor.clone(),
        );

        let error = runtime
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, GraphRunError::StateUpdateFailed { .. }));
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancellation_before_effect_dispatch_does_not_call_executor() {
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            single_node_graph(Behavior::CancelThenEffects(vec![TestEffect::Set(7)])),
            executor.clone(),
        );

        let error = runtime
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, GraphRunError::Cancelled));
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn usage_budget_failure_prevents_effect_dispatch() {
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            single_node_graph(Behavior::UsageThenEffects(
                UsageDelta { tokens: 1 },
                vec![TestEffect::Set(7)],
            )),
            executor.clone(),
        );

        let error = runtime
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8).with_tokens(0),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            GraphRunError::BudgetExceeded {
                resource: BudgetResource::Tokens,
                ..
            }
        ));
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn successful_effect_with_rejected_receipt_update_has_distinct_error() {
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            single_node_graph(Behavior::Effects(vec![TestEffect::RejectUpdate])),
            executor.clone(),
        );

        let error = runtime
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            error,
            GraphRunError::PostEffectStateUpdateFailed { .. }
        ));
    }

    #[tokio::test]
    async fn later_effect_failure_reports_already_completed_effect_ids() {
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            single_node_graph(Behavior::Effects(vec![
                TestEffect::Set(1),
                TestEffect::Fail,
            ])),
            executor.clone(),
        );

        let error = runtime
            .run(
                AgentState::new(TestBusiness::default()),
                RunBudget::for_test(8),
            )
            .await
            .unwrap_err();

        match error {
            GraphRunError::EffectFailed {
                effect_id,
                completed_effect_ids,
                ..
            } => {
                assert_eq!(completed_effect_ids.len(), 1);
                assert_eq!(completed_effect_ids[0].ordinal(), 0);
                assert_eq!(effect_id.ordinal(), 1);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cancellation_during_effect_execution_reports_unknown_commit() {
        let token = CancellationToken::new();
        let cancel = token.clone();
        let started = Arc::new(tokio::sync::Notify::new());
        let context = RunContext::new(RunBudget::for_test(8), token, RunTrace::default());
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            single_node_graph(Behavior::Effects(vec![TestEffect::Pending(
                started.clone(),
            )])),
            executor,
        );

        let mut run = tokio::spawn(async move {
            runtime
                .run_with_context(AgentState::new(TestBusiness::default()), context)
                .await
        });
        started.notified().await;
        cancel.cancel();

        let joined = match tokio::time::timeout(Duration::from_secs(1), &mut run).await {
            Ok(joined) => joined,
            Err(_) => {
                run.abort();
                panic!("runtime did not observe in-flight cancellation");
            }
        };
        let error = joined.unwrap().unwrap_err();

        match error {
            GraphRunError::EffectFailed { error, .. } => {
                assert_eq!(error.kind(), EffectErrorKind::UnknownCommit)
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn deadline_during_effect_execution_reports_unknown_commit() {
        let started = Arc::new(tokio::sync::Notify::new());
        let budget = RunBudget::new(NonZeroU32::new(8).unwrap(), Duration::from_millis(500));
        let context = RunContext::new(budget, CancellationToken::new(), RunTrace::default());
        let executor = Arc::new(TestEffectExecutor::recording());
        let runtime = GraphRuntime::with_effect_executor(
            single_node_graph(Behavior::Effects(vec![TestEffect::Pending(
                started.clone(),
            )])),
            executor,
        );

        let mut run = tokio::spawn(async move {
            runtime
                .run_with_context(AgentState::new(TestBusiness::default()), context)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("executor did not start before the Run deadline");

        let joined = match tokio::time::timeout(Duration::from_secs(2), &mut run).await {
            Ok(joined) => joined,
            Err(_) => {
                run.abort();
                panic!("runtime did not observe the in-flight deadline");
            }
        };
        let error = joined.unwrap().unwrap_err();

        match error {
            GraphRunError::EffectFailed { error, .. } => {
                assert_eq!(error.kind(), EffectErrorKind::UnknownCommit)
            }
            other => panic!("unexpected error: {other:?}"),
        }
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
