use super::{
    AgentNode, GraphBuildError, GraphCompileError, GraphId, GraphPolicy, GraphVersion, NodeId,
    RouteKey, TransitionRule,
};
use crate::AgentBusinessState;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::Arc;

/// Fragment 出口的结构化身份，避免点号拼接产生歧义。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct FragmentExitKey {
    namespace: String,
    name: String,
}

impl FragmentExitKey {
    pub(super) fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }
}

impl fmt::Display for FragmentExitKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.namespace, self.name)
    }
}

pub struct GraphDefinition<B: AgentBusinessState> {
    pub(super) id: GraphId,
    pub(super) version: GraphVersion,
    pub(super) nodes: BTreeMap<NodeId, Arc<dyn AgentNode<B>>>,
    pub(super) entry: Option<NodeId>,
    pub(super) transitions: BTreeMap<NodeId, TransitionRule<B>>,
    pub(super) mounted_namespaces: BTreeSet<String>,
    pub(super) unresolved_fragment_exits: BTreeMap<FragmentExitKey, (NodeId, RouteKey)>,
}

impl<B: AgentBusinessState> GraphDefinition<B> {
    pub fn new(id: GraphId) -> Self {
        Self::new_versioned(id, GraphVersion::initial())
    }

    pub fn new_versioned(id: GraphId, version: GraphVersion) -> Self {
        Self {
            id,
            version,
            nodes: BTreeMap::new(),
            entry: None,
            transitions: BTreeMap::new(),
            mounted_namespaces: BTreeSet::new(),
            unresolved_fragment_exits: BTreeMap::new(),
        }
    }

    pub fn id(&self) -> &GraphId {
        &self.id
    }

    pub fn version(&self) -> GraphVersion {
        self.version
    }

    pub fn add_node(&mut self, node: Arc<dyn AgentNode<B>>) -> Result<(), GraphBuildError> {
        let node_id = node.id().clone();
        if self.nodes.contains_key(&node_id) {
            return Err(GraphBuildError::DuplicateNode { node: node_id });
        }
        self.nodes.insert(node_id, node);
        Ok(())
    }

    pub fn contains_node(&self, node: &NodeId) -> bool {
        self.nodes.contains_key(node)
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

    pub fn compile(self, policy: GraphPolicy) -> Result<CompiledGraph<B>, GraphCompileError> {
        if let Some(exit) = self.unresolved_fragment_exits.keys().next() {
            return Err(GraphCompileError::UnresolvedFragmentExit {
                exit: exit.to_string(),
            });
        }
        let entry = self.entry.ok_or(GraphCompileError::MissingEntry)?;
        if !self.nodes.contains_key(&entry) {
            return Err(GraphCompileError::EntryNodeMissing { node: entry });
        }

        for node in self.nodes.keys() {
            if !self.transitions.contains_key(node) {
                return Err(GraphCompileError::MissingTransition { node: node.clone() });
            }
        }

        validate_transitions(&self.nodes, &self.transitions)?;
        validate_reachability(&entry, &self.nodes, &self.transitions)?;
        validate_terminal_paths(&self.nodes, &self.transitions)?;

        Ok(CompiledGraph {
            id: self.id,
            version: self.version,
            nodes: self.nodes,
            entry,
            transitions: self.transitions,
            policy,
        })
    }
}

pub struct CompiledGraph<B: AgentBusinessState> {
    id: GraphId,
    version: GraphVersion,
    nodes: BTreeMap<NodeId, Arc<dyn AgentNode<B>>>,
    entry: NodeId,
    transitions: BTreeMap<NodeId, TransitionRule<B>>,
    policy: GraphPolicy,
}

impl<B: AgentBusinessState> CompiledGraph<B> {
    pub fn id(&self) -> &GraphId {
        &self.id
    }

    pub fn version(&self) -> GraphVersion {
        self.version
    }

    pub fn entry(&self) -> &NodeId {
        &self.entry
    }

    pub fn policy(&self) -> GraphPolicy {
        self.policy
    }

    pub fn node_ids(&self) -> impl Iterator<Item = &NodeId> {
        self.nodes.keys()
    }

    pub(crate) fn node(&self, id: &NodeId) -> Option<&Arc<dyn AgentNode<B>>> {
        self.nodes.get(id)
    }

    pub(crate) fn transition(&self, id: &NodeId) -> Option<&TransitionRule<B>> {
        self.transitions.get(id)
    }
}

fn validate_transitions<B: AgentBusinessState>(
    nodes: &BTreeMap<NodeId, Arc<dyn AgentNode<B>>>,
    transitions: &BTreeMap<NodeId, TransitionRule<B>>,
) -> Result<(), GraphCompileError> {
    for (source, transition) in transitions {
        match transition {
            TransitionRule::Goto(target) => validate_target(nodes, source, target)?,
            TransitionRule::Branch { router, targets } => {
                if targets.is_empty() {
                    return Err(GraphCompileError::EmptyBranch {
                        node: source.clone(),
                    });
                }

                let known_routes = router.known_routes();
                let mut unique_routes = BTreeSet::new();
                for route in known_routes {
                    if !unique_routes.insert(route.clone()) {
                        return Err(GraphCompileError::DuplicateKnownRoute {
                            node: source.clone(),
                            route,
                        });
                    }
                }

                for route in &unique_routes {
                    if !targets.contains_key(route) {
                        return Err(GraphCompileError::MissingRouteTarget {
                            node: source.clone(),
                            route: route.clone(),
                        });
                    }
                }
                for (route, target) in targets {
                    if !unique_routes.contains(route) {
                        return Err(GraphCompileError::UnknownRouteTarget {
                            node: source.clone(),
                            route: route.clone(),
                        });
                    }
                    validate_target(nodes, source, target)?;
                }
            }
            TransitionRule::End => {}
        }
    }
    Ok(())
}

fn validate_target<B: AgentBusinessState>(
    nodes: &BTreeMap<NodeId, Arc<dyn AgentNode<B>>>,
    source: &NodeId,
    target: &NodeId,
) -> Result<(), GraphCompileError> {
    if nodes.contains_key(target) {
        Ok(())
    } else {
        Err(GraphCompileError::DanglingTarget {
            from: source.clone(),
            target: target.clone(),
        })
    }
}

fn validate_reachability<B: AgentBusinessState>(
    entry: &NodeId,
    nodes: &BTreeMap<NodeId, Arc<dyn AgentNode<B>>>,
    transitions: &BTreeMap<NodeId, TransitionRule<B>>,
) -> Result<(), GraphCompileError> {
    let mut reachable = BTreeSet::new();
    let mut queue = VecDeque::from([entry.clone()]);

    while let Some(node) = queue.pop_front() {
        if !reachable.insert(node.clone()) {
            continue;
        }
        for target in transition_targets(
            transitions
                .get(&node)
                .expect("all node transitions were validated"),
        ) {
            queue.push_back(target.clone());
        }
    }

    if let Some(node) = nodes.keys().find(|node| !reachable.contains(*node)) {
        return Err(GraphCompileError::UnreachableNode { node: node.clone() });
    }
    Ok(())
}

fn validate_terminal_paths<B: AgentBusinessState>(
    nodes: &BTreeMap<NodeId, Arc<dyn AgentNode<B>>>,
    transitions: &BTreeMap<NodeId, TransitionRule<B>>,
) -> Result<(), GraphCompileError> {
    let terminal_nodes: Vec<NodeId> = transitions
        .iter()
        .filter(|(_, transition)| matches!(transition, TransitionRule::End))
        .map(|(node, _)| node.clone())
        .collect();
    if terminal_nodes.is_empty() {
        return Err(GraphCompileError::NoTerminalPath);
    }

    let mut predecessors: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
    for (source, transition) in transitions {
        for target in transition_targets(transition) {
            predecessors
                .entry(target.clone())
                .or_default()
                .push(source.clone());
        }
    }

    let mut can_reach_end = BTreeSet::new();
    let mut queue = VecDeque::from(terminal_nodes);
    while let Some(node) = queue.pop_front() {
        if !can_reach_end.insert(node.clone()) {
            continue;
        }
        if let Some(previous) = predecessors.get(&node) {
            queue.extend(previous.iter().cloned());
        }
    }

    if let Some(node) = nodes.keys().find(|node| !can_reach_end.contains(*node)) {
        return Err(GraphCompileError::NodeCannotReachEnd { node: node.clone() });
    }
    Ok(())
}

fn transition_targets<B: AgentBusinessState>(transition: &TransitionRule<B>) -> Vec<&NodeId> {
    match transition {
        TransitionRule::Goto(target) => vec![target],
        TransitionRule::Branch { targets, .. } => targets.values().collect(),
        TransitionRule::End => Vec::new(),
    }
}
