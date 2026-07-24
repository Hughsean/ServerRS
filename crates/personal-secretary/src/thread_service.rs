use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    ClaimedThreadProjectionBatch, DeterministicThreadPlanner, InboundEventStoreError,
    ThreadProjectionLeaseToken, ThreadProjectionPlan, ThreadingError,
};

#[async_trait]
pub trait ThreadProjectionStoreT: Send + Sync {
    async fn claim_projection_batch(
        &self,
        max_events: u32,
        lease_secs: u64,
        same_conversation_window_secs: i64,
    ) -> Result<Option<ClaimedThreadProjectionBatch>, InboundEventStoreError>;

    async fn commit_projection(
        &self,
        plan: &ThreadProjectionPlan,
    ) -> Result<(), InboundEventStoreError>;

    async fn release_projection_claims(
        &self,
        lease_token: &ThreadProjectionLeaseToken,
        error: &str,
    ) -> Result<(), InboundEventStoreError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadProjectionRun {
    pub events_projected: usize,
    pub relations_created: usize,
    pub threads_created: usize,
}

pub struct ThreadProjectionUseCase {
    store: Arc<dyn ThreadProjectionStoreT>,
    planner: DeterministicThreadPlanner,
    batch_size: u32,
    lease_secs: u64,
    same_conversation_window_secs: i64,
}

impl ThreadProjectionUseCase {
    pub fn new(
        store: Arc<dyn ThreadProjectionStoreT>,
        planner: DeterministicThreadPlanner,
        batch_size: u32,
        lease_secs: u64,
        same_conversation_window_secs: i64,
    ) -> Result<Self, ThreadProjectionError> {
        if batch_size == 0 || batch_size > 1000 {
            return Err(ThreadProjectionError::InvalidConfiguration(
                "batch_size must be between 1 and 1000".into(),
            ));
        }
        if lease_secs == 0 || lease_secs > 3600 {
            return Err(ThreadProjectionError::InvalidConfiguration(
                "lease_secs must be between 1 and 3600".into(),
            ));
        }
        if same_conversation_window_secs <= 0 {
            return Err(ThreadProjectionError::InvalidConfiguration(
                "same_conversation_window_secs must be positive".into(),
            ));
        }
        Ok(Self {
            store,
            planner,
            batch_size,
            lease_secs,
            same_conversation_window_secs,
        })
    }

    pub async fn run_once(&self) -> Result<Option<ThreadProjectionRun>, ThreadProjectionError> {
        let Some(batch) = self
            .store
            .claim_projection_batch(
                self.batch_size,
                self.lease_secs,
                self.same_conversation_window_secs,
            )
            .await?
        else {
            return Ok(None);
        };
        let lease_token = batch.lease_token.clone();
        let plan = match self.planner.plan(batch) {
            Ok(plan) => plan,
            Err(error) => {
                let _ = self
                    .store
                    .release_projection_claims(&lease_token, &error.to_string())
                    .await;
                return Err(error.into());
            }
        };
        let run = ThreadProjectionRun {
            events_projected: plan.assignments.len(),
            relations_created: plan.relations.len(),
            threads_created: plan
                .assignments
                .iter()
                .filter(|assignment| assignment.creates_thread)
                .count(),
        };
        if let Err(error) = self.store.commit_projection(&plan).await {
            let _ = self
                .store
                .release_projection_claims(&lease_token, &error.to_string())
                .await;
            return Err(error.into());
        }
        Ok(Some(run))
    }
}

#[derive(Debug, Error)]
pub enum ThreadProjectionError {
    #[error("invalid thread projection configuration: {0}")]
    InvalidConfiguration(String),
    #[error(transparent)]
    Threading(#[from] ThreadingError),
    #[error(transparent)]
    Store(#[from] InboundEventStoreError),
}
