use super::{NodeError, NodeId, RouteKey, RunContext, UsageDelta};
use crate::domain::agent::{AgentBusinessState, AgentState, AgentUpdate};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;

#[async_trait]
pub trait AgentNode<B: AgentBusinessState>: Send + Sync {
    fn id(&self) -> &NodeId;

    async fn execute(
        &self,
        state: &AgentState<B>,
        context: &RunContext,
    ) -> Result<NodeResult<B::Update>, NodeError>;
}

pub trait Router<B: AgentBusinessState>: Send + Sync {
    fn known_routes(&self) -> Vec<RouteKey>;
    fn select(&self, state: &AgentState<B>) -> Result<RouteKey, NodeError>;
}

#[derive(Debug)]
pub struct NodeResult<U> {
    pub updates: Vec<AgentUpdate<U>>,
    pub usage: UsageDelta,
}

impl<U> NodeResult<U> {
    pub fn new(updates: Vec<AgentUpdate<U>>, usage: UsageDelta) -> Self {
        Self { updates, usage }
    }

    pub fn empty() -> Self {
        Self {
            updates: Vec::new(),
            usage: UsageDelta::default(),
        }
    }
}

pub enum TransitionRule<B: AgentBusinessState> {
    Goto(NodeId),
    Branch {
        router: Arc<dyn Router<B>>,
        targets: BTreeMap<RouteKey, NodeId>,
    },
    End,
}

impl<B: AgentBusinessState> Clone for TransitionRule<B> {
    fn clone(&self) -> Self {
        match self {
            Self::Goto(target) => Self::Goto(target.clone()),
            Self::Branch { router, targets } => Self::Branch {
                router: Arc::clone(router),
                targets: targets.clone(),
            },
            Self::End => Self::End,
        }
    }
}
