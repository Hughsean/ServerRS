mod budget;
mod checkpoint;
mod checkpoint_store;
mod definition;
mod effect;
mod error;
mod fragment;
mod id;
mod node;
mod runtime;

pub use crate::domain::agent::StateSchemaVersion;
pub use budget::{
    GraphPolicy, RunBudget, RunBudgetHandle, RunContext, RunTrace, UsageDelta, UsageSnapshot,
};
pub use checkpoint::{
    AgentCheckpoint, CheckpointError, CheckpointId, CheckpointRunError, GraphExecutionResult,
    GraphVersion, ResumeError, RunPosition, SuspendReason, SuspendRequest, SuspendedRun,
};
pub use checkpoint_store::{CheckpointStore, InMemoryCheckpointStore};
pub use definition::{CompiledGraph, GraphDefinition};
pub use effect::{
    AgentEffect, EffectEnvelope, EffectError, EffectErrorKind, EffectExecutor, EffectId,
    EffectReceipt, NoEffect, RunStep,
};
pub use error::{
    BudgetResource, GraphBuildError, GraphCompileError, GraphRunError, NodeError, NodeErrorKind,
};
pub use fragment::{FragmentExit, GraphFragment, MountedFragment};
pub use id::{GraphId, GraphIdError, NodeId, RouteKey, RunId};
pub use node::{AgentNode, NodeResult, Router, TransitionRule};
pub use runtime::{GraphRunResult, GraphRuntime};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent::{AgentBusinessState, AgentState, AgentStateError};
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::num::NonZeroU32;
    use std::sync::Arc;

    #[derive(Debug, Clone, Default)]
    struct TestBusiness;

    impl AgentBusinessState for TestBusiness {
        type Update = ();
        type Effect = NoEffect<()>;
        type SuspendData = ();
        type ResumeInput = ();

        fn resume_updates(_input: Self::ResumeInput) -> Vec<crate::domain::agent::AgentUpdate<()>> {
            Vec::new()
        }

        fn apply_update(&mut self, _update: Self::Update) -> Result<(), AgentStateError> {
            Ok(())
        }
    }

    struct NoopNode {
        id: NodeId,
    }

    impl NoopNode {
        fn new(id: &str) -> Self {
            Self { id: node_id(id) }
        }
    }

    #[async_trait]
    impl AgentNode<TestBusiness> for NoopNode {
        fn id(&self) -> &NodeId {
            &self.id
        }

        async fn execute(
            &self,
            _state: &AgentState<TestBusiness>,
            _context: &RunContext,
        ) -> Result<NodeResult<(), NoEffect<()>>, NodeError> {
            Ok(NodeResult::empty())
        }
    }

    struct StaticRouter {
        routes: Vec<RouteKey>,
    }

    impl Router<TestBusiness> for StaticRouter {
        fn known_routes(&self) -> Vec<RouteKey> {
            self.routes.clone()
        }

        fn select(&self, _state: &AgentState<TestBusiness>) -> Result<RouteKey, NodeError> {
            Ok(self.routes[0].clone())
        }
    }

    fn node_id(value: &str) -> NodeId {
        NodeId::try_from(value).unwrap()
    }

    fn route(value: &str) -> RouteKey {
        RouteKey::try_from(value).unwrap()
    }

    fn policy() -> GraphPolicy {
        GraphPolicy::new(NonZeroU32::new(8).unwrap())
    }

    fn graph() -> GraphDefinition<TestBusiness> {
        GraphDefinition::new(GraphId::try_from("test").unwrap())
    }

    #[test]
    fn node_id_rejects_spaces() {
        assert!(NodeId::try_from("bad node").is_err());
    }

    #[test]
    fn graph_id_rejects_values_longer_than_64_bytes() {
        assert!(GraphId::try_from("a".repeat(65).as_str()).is_err());
    }

    #[test]
    fn graph_rejects_duplicate_nodes() {
        let mut graph = graph();
        graph.add_node(Arc::new(NoopNode::new("start"))).unwrap();

        let error = graph
            .add_node(Arc::new(NoopNode::new("start")))
            .unwrap_err();

        assert!(matches!(error, GraphBuildError::DuplicateNode { .. }));
    }

    #[test]
    fn compile_rejects_missing_entry() {
        let mut graph = graph();
        graph.add_node(Arc::new(NoopNode::new("start"))).unwrap();
        graph
            .set_transition(node_id("start"), TransitionRule::End)
            .unwrap();

        assert!(matches!(
            graph.compile(policy()),
            Err(GraphCompileError::MissingEntry)
        ));
    }

    #[test]
    fn compile_rejects_dangling_target() {
        let mut graph = graph();
        graph.add_node(Arc::new(NoopNode::new("start"))).unwrap();
        graph.set_entry(node_id("start"));
        graph
            .set_transition(node_id("start"), TransitionRule::Goto(node_id("missing")))
            .unwrap();

        assert!(matches!(
            graph.compile(policy()),
            Err(GraphCompileError::DanglingTarget { .. })
        ));
    }

    #[test]
    fn compile_rejects_unreachable_nodes() {
        let mut graph = graph();
        graph.add_node(Arc::new(NoopNode::new("start"))).unwrap();
        graph.add_node(Arc::new(NoopNode::new("orphan"))).unwrap();
        graph.set_entry(node_id("start"));
        graph
            .set_transition(node_id("start"), TransitionRule::End)
            .unwrap();
        graph
            .set_transition(node_id("orphan"), TransitionRule::End)
            .unwrap();

        assert!(matches!(
            graph.compile(policy()),
            Err(GraphCompileError::UnreachableNode { .. })
        ));
    }

    #[test]
    fn compile_rejects_missing_known_branch_target() {
        let mut graph = graph();
        graph.add_node(Arc::new(NoopNode::new("start"))).unwrap();
        graph.add_node(Arc::new(NoopNode::new("finish"))).unwrap();
        graph.set_entry(node_id("start"));

        let router = StaticRouter {
            routes: vec![route("yes"), route("no")],
        };
        let targets = BTreeMap::from([(route("yes"), node_id("finish"))]);
        graph
            .set_transition(
                node_id("start"),
                TransitionRule::Branch {
                    router: Arc::new(router),
                    targets,
                },
            )
            .unwrap();
        graph
            .set_transition(node_id("finish"), TransitionRule::End)
            .unwrap();

        assert!(matches!(
            graph.compile(policy()),
            Err(GraphCompileError::MissingRouteTarget { .. })
        ));
    }

    #[test]
    fn compile_rejects_graph_without_end() {
        let mut graph = graph();
        graph.add_node(Arc::new(NoopNode::new("start"))).unwrap();
        graph.set_entry(node_id("start"));
        graph
            .set_transition(node_id("start"), TransitionRule::Goto(node_id("start")))
            .unwrap();

        assert!(matches!(
            graph.compile(policy()),
            Err(GraphCompileError::NoTerminalPath)
        ));
    }

    #[test]
    fn compile_accepts_a_bounded_cycle_with_an_exit() {
        let mut graph = graph();
        graph.add_node(Arc::new(NoopNode::new("loop"))).unwrap();
        graph.add_node(Arc::new(NoopNode::new("finish"))).unwrap();
        graph.set_entry(node_id("loop"));

        let router = StaticRouter {
            routes: vec![route("again"), route("done")],
        };
        let targets = BTreeMap::from([
            (route("again"), node_id("loop")),
            (route("done"), node_id("finish")),
        ]);
        graph
            .set_transition(
                node_id("loop"),
                TransitionRule::Branch {
                    router: Arc::new(router),
                    targets,
                },
            )
            .unwrap();
        graph
            .set_transition(node_id("finish"), TransitionRule::End)
            .unwrap();

        let compiled = graph.compile(policy()).unwrap();
        assert_eq!(compiled.entry(), &node_id("loop"));
        assert_eq!(compiled.policy().max_steps().get(), 8);
    }
}
