use super::{AgentCheckpoint, AgentEffect, CheckpointError, CheckpointId};
use crate::AgentBusinessState;
use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

#[async_trait]
pub trait CheckpointStore<B>: Send + Sync
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    async fn save(&self, checkpoint: AgentCheckpoint<B>) -> Result<(), CheckpointError>;

    async fn load(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<AgentCheckpoint<B>, CheckpointError>;

    async fn take(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<AgentCheckpoint<B>, CheckpointError>;
}

pub struct InMemoryCheckpointStore<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    state: Mutex<InMemoryCheckpointState<B>>,
}

struct InMemoryCheckpointState<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    checkpoints: BTreeMap<CheckpointId, AgentCheckpoint<B>>,
    consumed: BTreeSet<CheckpointId>,
}

impl<B> InMemoryCheckpointStore<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    pub fn new() -> Self {
        Self {
            state: Mutex::new(InMemoryCheckpointState {
                checkpoints: BTreeMap::new(),
                consumed: BTreeSet::new(),
            }),
        }
    }

    pub fn is_empty(&self) -> Result<bool, CheckpointError> {
        Ok(self.lock()?.checkpoints.is_empty())
    }

    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, InMemoryCheckpointState<B>>, CheckpointError> {
        self.state
            .lock()
            .map_err(|_| CheckpointError::StoreUnavailable)
    }
}

impl<B> Default for InMemoryCheckpointStore<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<B> CheckpointStore<B> for InMemoryCheckpointStore<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    async fn save(&self, checkpoint: AgentCheckpoint<B>) -> Result<(), CheckpointError> {
        let checkpoint_id = checkpoint.id();
        let mut state = self.lock()?;
        if state.checkpoints.contains_key(&checkpoint_id) || state.consumed.contains(&checkpoint_id)
        {
            return Err(CheckpointError::Duplicate { checkpoint_id });
        }
        state.checkpoints.insert(checkpoint_id, checkpoint);
        Ok(())
    }

    async fn load(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<AgentCheckpoint<B>, CheckpointError> {
        self.lock()?
            .checkpoints
            .get(&checkpoint_id)
            .cloned()
            .ok_or(CheckpointError::NotFound { checkpoint_id })
    }

    async fn take(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<AgentCheckpoint<B>, CheckpointError> {
        let mut state = self.lock()?;
        let checkpoint = state
            .checkpoints
            .remove(&checkpoint_id)
            .ok_or(CheckpointError::NotFound { checkpoint_id })?;
        state.consumed.insert(checkpoint_id);
        Ok(checkpoint)
    }
}
