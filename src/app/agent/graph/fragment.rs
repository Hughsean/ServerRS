use super::definition::FragmentExitKey;
use super::{
    AgentNode, GraphBuildError, GraphDefinition, NodeError, NodeId, NodeResult, RouteKey,
    RunContext, TransitionRule,
};
use crate::domain::agent::{AgentBusinessState, AgentState};
use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// 可复用的局部图。局部节点只有挂载后才获得全局命名空间。
pub struct GraphFragment<B: AgentBusinessState> {
    nodes: BTreeMap<NodeId, Arc<dyn AgentNode<B>>>,
    entry: Option<NodeId>,
    transitions: BTreeMap<NodeId, TransitionRule<B>>,
    exits: BTreeMap<String, LocalExit>,
}

struct LocalExit {
    source: NodeId,
    route: RouteKey,
}

impl<B: AgentBusinessState> GraphFragment<B> {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            entry: None,
            transitions: BTreeMap::new(),
            exits: BTreeMap::new(),
        }
    }

    pub fn add_node(&mut self, node: Arc<dyn AgentNode<B>>) -> Result<(), GraphBuildError> {
        let node_id = node.id().clone();
        if self.nodes.contains_key(&node_id) {
            return Err(GraphBuildError::DuplicateNode { node: node_id });
        }
        self.nodes.insert(node_id, node);
        Ok(())
    }

    pub fn set_entry(&mut self, entry: NodeId) {
        self.entry = Some(entry);
    }

    pub fn set_transition(
        &mut self,
        source: NodeId,
        transition: TransitionRule<B>,
    ) -> Result<(), GraphBuildError> {
        if !self.nodes.contains_key(&source) {
            return Err(GraphBuildError::UnknownTransitionSource { node: source });
        }
        if self.transitions.contains_key(&source) {
            return Err(GraphBuildError::DuplicateTransition { node: source });
        }
        self.transitions.insert(source, transition);
        Ok(())
    }

    pub fn declare_exit(
        &mut self,
        name: impl Into<String>,
        source: NodeId,
        route: RouteKey,
    ) -> Result<(), GraphBuildError> {
        let name = name.into();
        RouteKey::try_from(name.as_str()).map_err(|error| {
            GraphBuildError::InvalidFragmentExitName {
                name: name.clone(),
                error,
            }
        })?;
        if self.exits.contains_key(&name) {
            return Err(GraphBuildError::DuplicateFragmentExit { name });
        }
        self.exits.insert(name, LocalExit { source, route });
        Ok(())
    }
}

impl<B: AgentBusinessState> Default for GraphFragment<B> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct FragmentExit {
    key: FragmentExitKey,
    name: String,
    source: NodeId,
    route: RouteKey,
}

impl FragmentExit {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn source(&self) -> &NodeId {
        &self.source
    }

    pub fn route(&self) -> &RouteKey {
        &self.route
    }
}

#[derive(Debug)]
pub struct MountedFragment {
    namespace: String,
    entry: NodeId,
    exits: BTreeMap<String, FragmentExit>,
}

impl MountedFragment {
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn entry(&self) -> &NodeId {
        &self.entry
    }

    pub fn exit(&self, name: &str) -> Option<&FragmentExit> {
        self.exits.get(name)
    }

    pub fn exits(&self) -> impl Iterator<Item = &FragmentExit> {
        self.exits.values()
    }
}

impl<B: AgentBusinessState> GraphDefinition<B> {
    /// 将 Fragment 原子挂载到当前图中。
    pub fn mount(
        &mut self,
        namespace: &str,
        fragment: GraphFragment<B>,
    ) -> Result<MountedFragment, GraphBuildError> {
        NodeId::try_from(namespace).map_err(|error| GraphBuildError::InvalidNamespace {
            namespace: namespace.to_owned(),
            error,
        })?;
        if self.mounted_namespaces.contains(namespace) {
            return Err(GraphBuildError::DuplicateNamespace {
                namespace: namespace.to_owned(),
            });
        }
        validate_fragment(&fragment)?;

        let entry = fragment
            .entry
            .as_ref()
            .expect("fragment entry was validated");
        let mut id_map = BTreeMap::new();
        for local in fragment.nodes.keys() {
            let global_value = format!("{namespace}.{local}");
            let global = NodeId::try_from(global_value).map_err(|error| {
                GraphBuildError::InvalidNamespacedNodeId {
                    namespace: namespace.to_owned(),
                    local: local.clone(),
                    error,
                }
            })?;
            if self.nodes.contains_key(&global) {
                return Err(GraphBuildError::NamespaceCollision {
                    namespace: namespace.to_owned(),
                    node: global,
                });
            }
            id_map.insert(local.clone(), global);
        }

        let global_entry = id_map
            .get(entry)
            .expect("fragment entry mapping was validated")
            .clone();
        let mut mounted_nodes: BTreeMap<NodeId, Arc<dyn AgentNode<B>>> = BTreeMap::new();
        for (local, node) in fragment.nodes {
            let id = id_map
                .get(&local)
                .expect("all fragment nodes have a global mapping")
                .clone();
            mounted_nodes.insert(id.clone(), Arc::new(NamespacedNode { id, inner: node }));
        }

        let mut mounted_transitions = BTreeMap::new();
        for (local, transition) in fragment.transitions {
            let source = id_map
                .get(&local)
                .expect("all fragment transitions have a mapped source")
                .clone();
            mounted_transitions.insert(source, rewrite_transition(transition, &id_map));
        }

        let mut mounted_exits = BTreeMap::new();
        for (name, exit) in fragment.exits {
            let source = id_map
                .get(&exit.source)
                .expect("all fragment exits have a mapped source")
                .clone();
            let key = FragmentExitKey::new(namespace, name.as_str());
            mounted_exits.insert(
                name.clone(),
                FragmentExit {
                    key,
                    name,
                    source,
                    route: exit.route,
                },
            );
        }

        self.nodes.extend(mounted_nodes);
        self.transitions.extend(mounted_transitions);
        self.mounted_namespaces.insert(namespace.to_owned());
        for exit in mounted_exits.values() {
            self.unresolved_fragment_exits
                .insert(exit.key.clone(), (exit.source.clone(), exit.route.clone()));
        }

        Ok(MountedFragment {
            namespace: namespace.to_owned(),
            entry: global_entry,
            exits: mounted_exits,
        })
    }

    pub fn connect_exit(
        &mut self,
        exit: &FragmentExit,
        target: NodeId,
    ) -> Result<(), GraphBuildError> {
        let Some((source, route)) = self.unresolved_fragment_exits.get(&exit.key) else {
            return Err(GraphBuildError::UnknownFragmentExit {
                exit: exit.name.clone(),
            });
        };
        if source != &exit.source || route != &exit.route {
            return Err(GraphBuildError::UnknownFragmentExit {
                exit: exit.name.clone(),
            });
        }
        if !self.nodes.contains_key(&target) {
            return Err(GraphBuildError::UnknownFragmentExitTarget { node: target });
        }

        let transition = self
            .transitions
            .get_mut(&exit.source)
            .expect("mounted fragment exit source has a transition");
        let TransitionRule::Branch { targets, .. } = transition else {
            return Err(GraphBuildError::InvalidFragmentExitRoute {
                name: exit.name.clone(),
                node: exit.source.clone(),
                route: exit.route.clone(),
            });
        };
        if targets.contains_key(&exit.route) {
            return Err(GraphBuildError::UnknownFragmentExit {
                exit: exit.name.clone(),
            });
        }
        targets.insert(exit.route.clone(), target);
        self.unresolved_fragment_exits.remove(&exit.key);
        Ok(())
    }
}

struct NamespacedNode<B: AgentBusinessState> {
    id: NodeId,
    inner: Arc<dyn AgentNode<B>>,
}

#[async_trait]
impl<B: AgentBusinessState> AgentNode<B> for NamespacedNode<B> {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<B>,
        context: &RunContext,
    ) -> Result<NodeResult<B::Update>, NodeError> {
        self.inner.execute(state, context).await
    }
}

fn rewrite_transition<B: AgentBusinessState>(
    transition: TransitionRule<B>,
    id_map: &BTreeMap<NodeId, NodeId>,
) -> TransitionRule<B> {
    match transition {
        TransitionRule::Goto(target) => TransitionRule::Goto(
            id_map
                .get(&target)
                .expect("fragment transition target was validated")
                .clone(),
        ),
        TransitionRule::Branch { router, targets } => TransitionRule::Branch {
            router,
            targets: targets
                .into_iter()
                .map(|(route, target)| {
                    (
                        route,
                        id_map
                            .get(&target)
                            .expect("fragment branch target was validated")
                            .clone(),
                    )
                })
                .collect(),
        },
        TransitionRule::End => unreachable!("fragment End transition was validated"),
    }
}

fn validate_fragment<B: AgentBusinessState>(
    fragment: &GraphFragment<B>,
) -> Result<(), GraphBuildError> {
    let entry = fragment
        .entry
        .as_ref()
        .ok_or(GraphBuildError::FragmentMissingEntry)?;
    if !fragment.nodes.contains_key(entry) {
        return Err(GraphBuildError::FragmentEntryMissing {
            node: entry.clone(),
        });
    }
    if fragment.exits.is_empty() {
        return Err(GraphBuildError::FragmentMissingExit);
    }
    for node in fragment.nodes.keys() {
        if !fragment.transitions.contains_key(node) {
            return Err(GraphBuildError::FragmentMissingTransition { node: node.clone() });
        }
    }

    let mut exits_by_source: BTreeMap<&NodeId, BTreeSet<&RouteKey>> = BTreeMap::new();
    for (name, exit) in &fragment.exits {
        if !fragment.nodes.contains_key(&exit.source) {
            return Err(GraphBuildError::FragmentExitSourceMissing {
                name: name.clone(),
                node: exit.source.clone(),
            });
        }
        if !exits_by_source
            .entry(&exit.source)
            .or_default()
            .insert(&exit.route)
        {
            return Err(GraphBuildError::InvalidFragmentExitRoute {
                name: name.clone(),
                node: exit.source.clone(),
                route: exit.route.clone(),
            });
        }
    }

    for (source, transition) in &fragment.transitions {
        match transition {
            TransitionRule::Goto(target) => {
                validate_local_target(fragment, source, target)?;
                reject_exits_on_non_branch(fragment, source)?;
            }
            TransitionRule::Branch { router, targets } => {
                let mut known = BTreeSet::new();
                for route in router.known_routes() {
                    if !known.insert(route.clone()) {
                        return Err(GraphBuildError::DuplicateFragmentRoute {
                            node: source.clone(),
                            route,
                        });
                    }
                }
                for (route, target) in targets {
                    if !known.contains(route) {
                        return Err(GraphBuildError::UnknownFragmentRouteTarget {
                            node: source.clone(),
                            route: route.clone(),
                        });
                    }
                    validate_local_target(fragment, source, target)?;
                }

                let declared_exits = exits_by_source.get(source);
                for route in &known {
                    let is_internal = targets.contains_key(route);
                    let is_exit = declared_exits.is_some_and(|routes| routes.contains(route));
                    if !is_internal && !is_exit {
                        return Err(GraphBuildError::UnresolvedFragmentRoute {
                            node: source.clone(),
                            route: route.clone(),
                        });
                    }
                }
                if let Some(exits) = declared_exits {
                    for route in exits {
                        if !known.contains(*route) || targets.contains_key(*route) {
                            let name = fragment
                                .exits
                                .iter()
                                .find(|(_, exit)| &exit.source == source && &exit.route == *route)
                                .map(|(name, _)| name.clone())
                                .expect("declared exit name exists");
                            return Err(GraphBuildError::InvalidFragmentExitRoute {
                                name,
                                node: source.clone(),
                                route: (*route).clone(),
                            });
                        }
                    }
                }
            }
            TransitionRule::End => {
                return Err(GraphBuildError::FragmentContainsEnd {
                    node: source.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_local_target<B: AgentBusinessState>(
    fragment: &GraphFragment<B>,
    source: &NodeId,
    target: &NodeId,
) -> Result<(), GraphBuildError> {
    if fragment.nodes.contains_key(target) {
        Ok(())
    } else {
        Err(GraphBuildError::FragmentDanglingTarget {
            from: source.clone(),
            target: target.clone(),
        })
    }
}

fn reject_exits_on_non_branch<B: AgentBusinessState>(
    fragment: &GraphFragment<B>,
    source: &NodeId,
) -> Result<(), GraphBuildError> {
    if let Some((name, exit)) = fragment
        .exits
        .iter()
        .find(|(_, exit)| &exit.source == source)
    {
        return Err(GraphBuildError::InvalidFragmentExitRoute {
            name: name.clone(),
            node: source.clone(),
            route: exit.route.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::domain::agent::{AgentBusinessState, AgentState, AgentStateError};
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::num::NonZeroU32;
    use std::sync::Arc;

    #[derive(Debug, Clone, Default)]
    struct TestBusiness;

    impl AgentBusinessState for TestBusiness {
        type Update = ();

        fn apply_update(&mut self, _update: Self::Update) -> Result<(), AgentStateError> {
            Ok(())
        }
    }

    struct NoopNode {
        id: NodeId,
    }

    impl NoopNode {
        fn new(id: &str) -> Self {
            Self {
                id: NodeId::try_from(id).unwrap(),
            }
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
        ) -> Result<NodeResult<()>, NodeError> {
            Ok(NodeResult::empty())
        }
    }

    struct LoopRouter;

    impl Router<TestBusiness> for LoopRouter {
        fn known_routes(&self) -> Vec<RouteKey> {
            vec![route("tools_requested"), route("final_response")]
        }

        fn select(&self, _state: &AgentState<TestBusiness>) -> Result<RouteKey, NodeError> {
            Ok(route("final_response"))
        }
    }

    fn id(value: &str) -> NodeId {
        NodeId::try_from(value).unwrap()
    }

    fn route(value: &str) -> RouteKey {
        RouteKey::try_from(value).unwrap()
    }

    fn policy() -> GraphPolicy {
        GraphPolicy::new(NonZeroU32::new(8).unwrap())
    }

    fn fragment() -> GraphFragment<TestBusiness> {
        let mut fragment = GraphFragment::new();
        fragment.add_node(Arc::new(NoopNode::new("llm"))).unwrap();
        fragment.add_node(Arc::new(NoopNode::new("tools"))).unwrap();
        fragment.set_entry(id("llm"));
        fragment
            .set_transition(
                id("llm"),
                TransitionRule::Branch {
                    router: Arc::new(LoopRouter),
                    targets: BTreeMap::from([(route("tools_requested"), id("tools"))]),
                },
            )
            .unwrap();
        fragment
            .set_transition(id("tools"), TransitionRule::Goto(id("llm")))
            .unwrap();
        fragment
            .declare_exit("final_response", id("llm"), route("final_response"))
            .unwrap();
        fragment
    }

    fn fragment_with_exit_name(exit_name: &str) -> GraphFragment<TestBusiness> {
        let mut fragment = fragment();
        let exit = fragment
            .exits
            .remove("final_response")
            .expect("default fragment exit");
        fragment
            .declare_exit(exit_name, exit.source, exit.route)
            .unwrap();
        fragment
    }

    fn parent() -> GraphDefinition<TestBusiness> {
        GraphDefinition::new(GraphId::try_from("parent").unwrap())
    }

    #[test]
    fn mounting_fragment_namespaces_all_internal_nodes() {
        let mut parent = parent();
        let mounted = parent.mount("reasoning", fragment()).unwrap();

        assert_eq!(mounted.entry(), &id("reasoning.llm"));
        assert_eq!(
            mounted.exit("final_response").unwrap().source(),
            &id("reasoning.llm")
        );
        assert!(parent.contains_node(&id("reasoning.llm")));
        assert!(parent.contains_node(&id("reasoning.tools")));
        assert!(mounted.exit("tools").is_none());
    }

    #[test]
    fn mounting_the_same_namespace_twice_is_rejected() {
        let mut parent = parent();
        parent.mount("reasoning", fragment()).unwrap();

        let error = parent.mount("reasoning", fragment()).unwrap_err();

        assert!(matches!(error, GraphBuildError::DuplicateNamespace { .. }));
    }

    #[test]
    fn namespace_collision_does_not_partially_mount_fragment() {
        let mut parent = parent();
        parent
            .add_node(Arc::new(NoopNode::new("reasoning.llm")))
            .unwrap();

        let error = parent.mount("reasoning", fragment()).unwrap_err();

        assert!(matches!(error, GraphBuildError::NamespaceCollision { .. }));
        assert!(!parent.contains_node(&id("reasoning.tools")));
    }

    #[test]
    fn dotted_namespace_and_exit_names_do_not_alias_each_other() {
        let mut parent = parent();
        parent.add_node(Arc::new(NoopNode::new("finish"))).unwrap();
        let first = parent.mount("a.b", fragment_with_exit_name("c")).unwrap();
        let second = parent.mount("a", fragment_with_exit_name("b.c")).unwrap();

        parent
            .connect_exit(first.exit("c").unwrap(), id("finish"))
            .unwrap();
        parent
            .connect_exit(second.exit("b.c").unwrap(), id("finish"))
            .unwrap();

        assert!(parent.unresolved_fragment_exits.is_empty());
    }

    #[test]
    fn compile_rejects_unconnected_fragment_exit() {
        let mut parent = parent();
        let mounted = parent.mount("reasoning", fragment()).unwrap();
        parent.set_entry(mounted.entry().clone());

        assert!(matches!(
            parent.compile(policy()),
            Err(GraphCompileError::UnresolvedFragmentExit { .. })
        ));
    }

    #[test]
    fn connecting_every_exit_produces_a_compilable_parent_graph() {
        let mut parent = parent();
        parent.add_node(Arc::new(NoopNode::new("finish"))).unwrap();
        let mounted = parent.mount("reasoning", fragment()).unwrap();
        parent
            .connect_exit(mounted.exit("final_response").unwrap(), id("finish"))
            .unwrap();
        parent.set_entry(mounted.entry().clone());
        parent
            .set_transition(id("finish"), TransitionRule::End)
            .unwrap();

        let compiled = parent.compile(policy()).unwrap();
        let node_ids: Vec<_> = compiled.node_ids().cloned().collect();
        assert_eq!(
            node_ids,
            vec![id("finish"), id("reasoning.llm"), id("reasoning.tools")]
        );
    }

    #[test]
    fn an_exit_cannot_be_connected_twice() {
        let mut parent = parent();
        parent.add_node(Arc::new(NoopNode::new("finish"))).unwrap();
        let mounted = parent.mount("reasoning", fragment()).unwrap();
        let exit = mounted.exit("final_response").unwrap();
        parent.connect_exit(exit, id("finish")).unwrap();

        let error = parent.connect_exit(exit, id("finish")).unwrap_err();
        assert!(matches!(error, GraphBuildError::UnknownFragmentExit { .. }));
    }
}
