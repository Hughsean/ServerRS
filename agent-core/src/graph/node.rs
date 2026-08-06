use super::{NodeError, NodeId, RouteKey, RunContext, SuspendRequest, UsageDelta};
use crate::{AgentBusinessState, AgentState, AgentUpdate};
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
    ) -> Result<NodeResult<B::Update, B::Effect, B::SuspendData>, NodeError>;
}

pub trait Router<B: AgentBusinessState>: Send + Sync {
    fn known_routes(&self) -> Vec<RouteKey>;
    fn select(&self, state: &AgentState<B>) -> Result<RouteKey, NodeError>;
}

#[derive(Debug)]
pub enum NodeResult<U, E, S = ()> {
    Continue {
        updates: Vec<AgentUpdate<U>>,
        effects: Vec<E>,
        usage: UsageDelta,
    },
    Suspend {
        updates: Vec<AgentUpdate<U>>,
        effects: Vec<E>,
        usage: UsageDelta,
        request: SuspendRequest<S>,
    },
}

impl<U, E, S> NodeResult<U, E, S> {
    pub fn updates(&self) -> &[AgentUpdate<U>] {
        match self {
            Self::Continue { updates, .. } | Self::Suspend { updates, .. } => updates,
        }
    }

    pub fn effects(&self) -> &[E] {
        match self {
            Self::Continue { effects, .. } | Self::Suspend { effects, .. } => effects,
        }
    }

    pub fn into_updates(self) -> Vec<AgentUpdate<U>> {
        match self {
            Self::Continue { updates, .. } | Self::Suspend { updates, .. } => updates,
        }
    }

    pub fn new(updates: Vec<AgentUpdate<U>>, usage: UsageDelta) -> Self {
        Self::Continue {
            updates,
            effects: Vec::new(),
            usage,
        }
    }

    pub fn empty() -> Self {
        Self::Continue {
            updates: Vec::new(),
            effects: Vec::new(),
            usage: UsageDelta::default(),
        }
    }

    pub fn with_effect(updates: Vec<AgentUpdate<U>>, effect: E, usage: UsageDelta) -> Self {
        Self::Continue {
            updates,
            effects: vec![effect],
            usage,
        }
    }

    pub fn with_effects(updates: Vec<AgentUpdate<U>>, effects: Vec<E>, usage: UsageDelta) -> Self {
        Self::Continue {
            updates,
            effects,
            usage,
        }
    }

    pub fn suspend(
        updates: Vec<AgentUpdate<U>>,
        effects: Vec<E>,
        usage: UsageDelta,
        request: SuspendRequest<S>,
    ) -> Self {
        Self::Suspend {
            updates,
            effects,
            usage,
            request,
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
