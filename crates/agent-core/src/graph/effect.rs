use super::{NodeId, RunContext, RunId};
use crate::AgentUpdate;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::error::Error as StdError;
use std::fmt::{Debug, Display, Formatter};
use std::marker::PhantomData;
use std::num::NonZeroU32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunStep(NonZeroU32);

impl RunStep {
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl TryFrom<u32> for RunStep {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or("RunStep 必须从 1 开始")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EffectId {
    run_id: RunId,
    step: RunStep,
    node_id: NodeId,
    ordinal: u32,
}

impl EffectId {
    pub fn new(run_id: RunId, step: RunStep, node_id: NodeId, ordinal: u32) -> Self {
        Self {
            run_id,
            step,
            node_id,
            ordinal,
        }
    }

    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    pub fn step(&self) -> RunStep {
        self.step
    }

    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }
}

impl Display for EffectId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}/{}/{}/{}",
            self.run_id,
            self.step.get(),
            self.node_id,
            self.ordinal
        )
    }
}

pub trait AgentEffect: Send + Sync + 'static {
    type Update: Send + Sync + 'static;
    type Receipt: Clone + Send + Sync + 'static;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>>;
}

#[derive(Debug)]
pub struct EffectEnvelope<E> {
    pub id: EffectId,
    pub effect: E,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectReceipt<R> {
    pub effect_id: EffectId,
    pub value: R,
}

#[async_trait]
pub trait EffectExecutor<E: AgentEffect>: Send + Sync {
    async fn execute(
        &self,
        envelope: &EffectEnvelope<E>,
        context: &RunContext,
    ) -> Result<E::Receipt, EffectError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectErrorKind {
    Transient,
    Permanent,
    Timeout,
    UnknownCommit,
}

#[derive(Debug, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct EffectError {
    kind: EffectErrorKind,
    message: String,
    #[source]
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl EffectError {
    pub fn new(kind: EffectErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    pub fn unknown_commit(message: impl Into<String>) -> Self {
        Self::new(EffectErrorKind::UnknownCommit, message)
    }

    pub fn with_source<E>(kind: EffectErrorKind, error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self {
            kind,
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }

    pub fn kind(&self) -> EffectErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn source_ref<E>(&self) -> Option<&E>
    where
        E: StdError + Send + Sync + 'static,
    {
        self.source.as_deref()?.downcast_ref::<E>()
    }
}

pub enum NoEffect<U> {
    #[doc(hidden)]
    Never(Infallible, PhantomData<fn() -> U>),
}

impl<U> Debug for NoEffect<U> {
    fn fmt(&self, _formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Never(value, _) => match *value {},
        }
    }
}

impl<U: Send + Sync + 'static> AgentEffect for NoEffect<U> {
    type Update = U;
    type Receipt = Infallible;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>> {
        match *receipt {}
    }
}
