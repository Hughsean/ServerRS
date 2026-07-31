use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    ConversationMemoryModeInput, ConversationMemoryModeReceipt, InboundEventStoreError,
    MemoryDeleteInput, MemoryDeleteReceipt, MemoryFact, MemoryFactError, MemoryFactId,
    MemoryFactView, MemoryWriteReceipt, SourceAccountRef, validate_memory_delete,
    validate_memory_fact,
};

#[async_trait]
pub trait MemoryStoreT: Send + Sync {
    async fn append_fact(
        &self,
        fact: &MemoryFact,
    ) -> Result<MemoryWriteReceipt, InboundEventStoreError>;

    async fn list_active(
        &self,
        account: &SourceAccountRef,
        limit: u32,
    ) -> Result<Vec<MemoryFact>, InboundEventStoreError>;

    async fn expire_due(
        &self,
        now_unix_secs: i64,
        limit: u32,
    ) -> Result<u64, InboundEventStoreError>;

    async fn load_with_sources(
        &self,
        fact_id: &MemoryFactId,
        max_excerpt_chars: u32,
    ) -> Result<Option<MemoryFactView>, InboundEventStoreError>;

    async fn delete_derived(
        &self,
        input: &MemoryDeleteInput,
    ) -> Result<MemoryDeleteReceipt, InboundEventStoreError>;

    async fn set_conversation_mode(
        &self,
        input: &ConversationMemoryModeInput,
    ) -> Result<ConversationMemoryModeReceipt, InboundEventStoreError>;
}

pub struct MemoryUseCase {
    store: Arc<dyn MemoryStoreT>,
}

impl MemoryUseCase {
    pub fn new(store: Arc<dyn MemoryStoreT>) -> Self {
        Self { store }
    }

    pub async fn remember(
        &self,
        fact: &MemoryFact,
    ) -> Result<MemoryWriteReceipt, MemoryUseCaseError> {
        validate_memory_fact(fact)?;
        Ok(self.store.append_fact(fact).await?)
    }

    pub async fn active(
        &self,
        account: &SourceAccountRef,
        limit: u32,
    ) -> Result<Vec<MemoryFact>, MemoryUseCaseError> {
        if !(1..=200).contains(&limit) {
            return Err(MemoryUseCaseError::InvalidLimit);
        }
        Ok(self.store.list_active(account, limit).await?)
    }

    pub async fn evidence(
        &self,
        fact_id: &MemoryFactId,
        max_excerpt_chars: u32,
    ) -> Result<Option<MemoryFactView>, MemoryUseCaseError> {
        if !(1..=2000).contains(&max_excerpt_chars) {
            return Err(MemoryUseCaseError::InvalidExcerptLimit);
        }
        Ok(self
            .store
            .load_with_sources(fact_id, max_excerpt_chars)
            .await?)
    }

    pub async fn delete_derived(
        &self,
        input: &MemoryDeleteInput,
    ) -> Result<MemoryDeleteReceipt, MemoryUseCaseError> {
        validate_memory_delete(input)?;
        Ok(self.store.delete_derived(input).await?)
    }

    pub async fn set_conversation_mode(
        &self,
        input: &ConversationMemoryModeInput,
    ) -> Result<ConversationMemoryModeReceipt, MemoryUseCaseError> {
        Ok(self.store.set_conversation_mode(input).await?)
    }
}

#[derive(Debug, Error)]
pub enum MemoryUseCaseError {
    #[error("memory query limit must be in 1..=200")]
    InvalidLimit,
    #[error("memory source excerpt limit must be in 1..=2000")]
    InvalidExcerptLimit,
    #[error(transparent)]
    Domain(#[from] MemoryFactError),
    #[error(transparent)]
    Store(#[from] InboundEventStoreError),
}
