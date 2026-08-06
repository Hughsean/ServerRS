use super::{
    AgentEffect, EffectReceipt, GraphId, GraphRunError, GraphRunResult, NodeId, RunBudget, RunId,
    RunStep, RunTrace, UsageSnapshot,
};
use crate::{AgentBusinessState, AgentState, AgentStateError, StateSchemaVersion};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter};
use std::num::NonZeroU32;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CheckpointId(Uuid);

impl CheckpointId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
}

impl Default for CheckpointId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for CheckpointId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl FromStr for CheckpointId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GraphVersion(NonZeroU32);

impl GraphVersion {
    pub const fn initial() -> Self {
        Self(NonZeroU32::MIN)
    }

    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl TryFrom<u32> for GraphVersion {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or("GraphVersion 必须大于 0")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspendReason {
    ExternalInput,
    Approval,
    ExternalEvent,
    Business,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuspendRequest<S> {
    pub reason: SuspendReason,
    pub data: S,
}

impl<S> SuspendRequest<S> {
    pub fn new(reason: SuspendReason, data: S) -> Self {
        Self { reason, data }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPosition {
    completed_step: RunStep,
    next_node: NodeId,
}

impl RunPosition {
    pub fn new(completed_step: RunStep, next_node: NodeId) -> Self {
        Self {
            completed_step,
            next_node,
        }
    }

    pub fn completed_step(&self) -> RunStep {
        self.completed_step
    }

    pub fn next_node(&self) -> &NodeId {
        &self.next_node
    }
}

#[derive(Serialize, Deserialize)]
#[serde(bound(
    serialize = "B: Serialize, B::SuspendData: Serialize, <B::Effect as AgentEffect>::Receipt: Serialize",
    deserialize = "B: Deserialize<'de>, B::SuspendData: Deserialize<'de>, <B::Effect as AgentEffect>::Receipt: Deserialize<'de>"
))]
pub struct AgentCheckpoint<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    id: CheckpointId,
    graph_id: GraphId,
    graph_version: GraphVersion,
    state_schema_version: StateSchemaVersion,
    run_id: RunId,
    position: RunPosition,
    state: AgentState<B>,
    budget: RunBudget,
    usage: UsageSnapshot,
    visited: Vec<NodeId>,
    effect_receipts: Vec<EffectReceipt<<B::Effect as AgentEffect>::Receipt>>,
    suspend: SuspendRequest<B::SuspendData>,
    trace: RunTrace,
}

impl<B> Clone for AgentCheckpoint<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            graph_id: self.graph_id.clone(),
            graph_version: self.graph_version,
            state_schema_version: self.state_schema_version,
            run_id: self.run_id,
            position: self.position.clone(),
            state: self.state.clone(),
            budget: self.budget,
            usage: self.usage,
            visited: self.visited.clone(),
            effect_receipts: self.effect_receipts.clone(),
            suspend: self.suspend.clone(),
            trace: self.trace.clone(),
        }
    }
}

impl<B> AgentCheckpoint<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: CheckpointId,
        graph_id: GraphId,
        graph_version: GraphVersion,
        state_schema_version: StateSchemaVersion,
        run_id: RunId,
        position: RunPosition,
        state: AgentState<B>,
        budget: RunBudget,
        usage: UsageSnapshot,
        visited: Vec<NodeId>,
        effect_receipts: Vec<EffectReceipt<<B::Effect as AgentEffect>::Receipt>>,
        suspend: SuspendRequest<B::SuspendData>,
        trace: RunTrace,
    ) -> Self {
        Self {
            id,
            graph_id,
            graph_version,
            state_schema_version,
            run_id,
            position,
            state,
            budget,
            usage,
            visited,
            effect_receipts,
            suspend,
            trace,
        }
    }

    pub fn id(&self) -> CheckpointId {
        self.id
    }

    pub fn graph_id(&self) -> &GraphId {
        &self.graph_id
    }

    pub fn graph_version(&self) -> GraphVersion {
        self.graph_version
    }

    pub fn state_schema_version(&self) -> StateSchemaVersion {
        self.state_schema_version
    }

    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    pub fn position(&self) -> &RunPosition {
        &self.position
    }

    pub fn state(&self) -> &AgentState<B> {
        &self.state
    }

    pub fn budget(&self) -> RunBudget {
        self.budget
    }

    pub fn usage(&self) -> UsageSnapshot {
        self.usage
    }

    pub fn visited(&self) -> &[NodeId] {
        &self.visited
    }

    pub fn effect_receipts(&self) -> &[EffectReceipt<<B::Effect as AgentEffect>::Receipt>] {
        &self.effect_receipts
    }

    pub fn suspend(&self) -> &SuspendRequest<B::SuspendData> {
        &self.suspend
    }

    pub fn trace(&self) -> &RunTrace {
        &self.trace
    }
}

impl<B> Debug for AgentCheckpoint<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentCheckpoint")
            .field("id", &self.id)
            .field("graph_id", &self.graph_id)
            .field("graph_version", &self.graph_version)
            .field("state_schema_version", &self.state_schema_version)
            .field("run_id", &self.run_id)
            .field("position", &self.position)
            .field("usage", &self.usage)
            .field("visited", &self.visited)
            .field("effect_receipt_count", &self.effect_receipts.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum CheckpointError {
    #[error("Checkpoint 不存在: {checkpoint_id}")]
    NotFound { checkpoint_id: CheckpointId },
    #[error("Checkpoint ID 已存在: {checkpoint_id}")]
    Duplicate { checkpoint_id: CheckpointId },
    #[error("CheckpointStore 内部状态不可用")]
    StoreUnavailable,
}

#[derive(Debug)]
pub struct SuspendedRun<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    checkpoint: AgentCheckpoint<B>,
}

impl<B> SuspendedRun<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    pub fn new(checkpoint: AgentCheckpoint<B>) -> Self {
        Self { checkpoint }
    }

    pub fn checkpoint(&self) -> &AgentCheckpoint<B> {
        &self.checkpoint
    }
}

#[derive(Debug)]
pub enum GraphExecutionResult<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    Completed(GraphRunResult<B>),
    Suspended(SuspendedRun<B>),
}

#[derive(Debug, thiserror::Error)]
pub enum CheckpointRunError<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    #[error(transparent)]
    Graph(#[from] GraphRunError),
    #[error("图运行器未配置 CheckpointStore")]
    MissingStore,
    #[error("保存 Checkpoint {checkpoint_id} 失败: {source}")]
    SaveFailed {
        checkpoint_id: CheckpointId,
        checkpoint: Box<AgentCheckpoint<B>>,
        #[source]
        source: CheckpointError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ResumeError<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    #[error("加载 Checkpoint 失败: {source}")]
    CheckpointLoad {
        #[source]
        source: CheckpointError,
    },
    #[error("Checkpoint GraphId 不匹配：期望 {expected}，实际 {actual}")]
    GraphIdMismatch { expected: GraphId, actual: GraphId },
    #[error("Checkpoint GraphVersion 不匹配：期望 {expected:?}，实际 {actual:?}")]
    GraphVersionMismatch {
        expected: GraphVersion,
        actual: GraphVersion,
    },
    #[error("Checkpoint StateSchemaVersion 不匹配：期望 {expected:?}，实际 {actual:?}")]
    StateSchemaVersionMismatch {
        expected: StateSchemaVersion,
        actual: StateSchemaVersion,
    },
    #[error(
        "Checkpoint RunPosition 与 Usage 不一致：完成步骤 {completed_step}，Usage 步骤 {usage_steps}"
    )]
    RunPositionMismatch {
        completed_step: u32,
        usage_steps: u32,
    },
    #[error("Checkpoint 的下一节点不存在: {node}")]
    MissingNode { node: NodeId },
    #[error("ResumeInput 产生的状态更新被拒绝: {error}")]
    ResumeInputRejected {
        #[source]
        error: AgentStateError,
    },
    #[error("Checkpoint 已被其他恢复操作消费: {checkpoint_id}")]
    CheckpointAlreadyConsumed { checkpoint_id: CheckpointId },
    #[error("恢复执行失败: {source}")]
    RunFailed {
        #[source]
        source: Box<CheckpointRunError<B>>,
    },
}
