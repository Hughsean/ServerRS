//! Agenda 用例与持久化端口。

use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    AgendaError, AgendaItem, AgendaMutation, Clock, SourceAccountRef, SourceEventId,
    validate_agenda_mutation,
};

#[derive(Debug, Clone)]
pub struct AgendaApplyRequest {
    pub account: SourceAccountRef,
    pub command_source_event_id: SourceEventId,
    pub run_id: String,
    pub effect_id: String,
    pub proposal_id: String,
    pub proposal_json: String,
    pub lease_token: String,
    pub idempotency_key: String,
    pub mutation: AgendaMutation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgendaMutationReceipt {
    pub item: AgendaItem,
    pub result_ref: String,
}

#[async_trait]
pub trait AgendaStoreT: Send + Sync {
    /// Agenda mutation、不可变审计和 Action Effect Receipt 必须在同一事务中提交。
    async fn apply(
        &self,
        request: &AgendaApplyRequest,
        now_unix_secs: i64,
    ) -> Result<AgendaMutationReceipt, AgendaError>;

    async fn list_upcoming(
        &self,
        account: &SourceAccountRef,
        now_unix_secs: i64,
        horizon_secs: u64,
        limit: u32,
    ) -> Result<Vec<AgendaItem>, AgendaError>;

    async fn enqueue_due_notifications(
        &self,
        now_unix_secs: i64,
        limit: u32,
    ) -> Result<u64, AgendaError>;
}

pub struct AgendaUseCase {
    store: Arc<dyn AgendaStoreT>,
    clock: Arc<dyn Clock>,
}

impl AgendaUseCase {
    pub fn new(store: Arc<dyn AgendaStoreT>, clock: Arc<dyn Clock>) -> Self {
        Self { store, clock }
    }

    pub async fn apply(
        &self,
        request: &AgendaApplyRequest,
    ) -> Result<AgendaMutationReceipt, AgendaError> {
        if request.run_id.trim().is_empty()
            || request.effect_id.trim().is_empty()
            || request.idempotency_key.trim().is_empty()
            || request.idempotency_key.len() > 191
        {
            return Err(AgendaError::Invalid(
                "agenda effect identity is missing or unbounded".into(),
            ));
        }
        let now = self.clock.now_unix_secs();
        validate_agenda_mutation(&request.mutation, now)?;
        self.store.apply(request, now).await
    }

    pub async fn list_upcoming(
        &self,
        account: &SourceAccountRef,
        horizon_secs: u64,
    ) -> Result<Vec<AgendaItem>, AgendaError> {
        if !(1..=31_536_000).contains(&horizon_secs) {
            return Err(AgendaError::Invalid(
                "agenda horizon must be in 1..=31536000 seconds".into(),
            ));
        }
        self.store
            .list_upcoming(account, self.clock.now_unix_secs(), horizon_secs, 100)
            .await
    }

    pub async fn enqueue_due_notifications(&self, limit: u32) -> Result<u64, AgendaError> {
        if !(1..=1000).contains(&limit) {
            return Err(AgendaError::Invalid(
                "agenda notification scan limit must be in 1..=1000".into(),
            ));
        }
        self.store
            .enqueue_due_notifications(self.clock.now_unix_secs(), limit)
            .await
    }
}
