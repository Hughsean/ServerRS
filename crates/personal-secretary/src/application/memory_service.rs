use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    ActionLeaseToken, ActionRunId, ConversationMemoryModeInput, ConversationMemoryModeReceipt,
    InboundEventStoreError, MemoryDeleteInput, MemoryDeleteReceipt, MemoryFact, MemoryFactError,
    MemoryFactId, MemoryFactView, MemoryWriteReceipt, SecretaryAction, SecretaryActionProposal,
    SecretaryActionReceipt, SourceAccountRef, SourceEventId, validate_memory_delete,
    validate_memory_fact,
};

/// Owner 记忆写命令的最终 Effect 边界。基础设施实现必须在同一事务内复验
/// Action 租约、原始 OwnerCommand、active OwnerBinding 和账号，并原子提交业务变更与 Receipt。
#[derive(Debug, Clone)]
pub struct MemoryEffectRequest {
    pub account: SourceAccountRef,
    pub command_source_event_id: SourceEventId,
    pub run_id: ActionRunId,
    pub lease_token: ActionLeaseToken,
    pub effect_id: String,
    pub proposal_id: String,
    pub proposal_json: String,
    pub action: SecretaryAction,
    pub now_unix_secs: i64,
}

#[derive(Debug, Error)]
pub enum MemoryEffectStoreError {
    #[error("memory effect is unauthorized")]
    Unauthorized,
    #[error("memory effect lease was lost")]
    LeaseLost,
    #[error("memory effect data is invalid: {0}")]
    InvalidData(String),
    #[error("memory effect database operation failed")]
    Database,
}

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

    async fn apply_owner_effect(
        &self,
        request: &MemoryEffectRequest,
    ) -> Result<SecretaryActionReceipt, MemoryEffectStoreError>;
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

    pub async fn apply_owner_effect(
        &self,
        request: &MemoryEffectRequest,
    ) -> Result<SecretaryActionReceipt, MemoryEffectStoreError> {
        if request.effect_id.trim().is_empty()
            || request.effect_id.len() > 255
            || request.proposal_id.trim().is_empty()
            || request.proposal_json.len() > 65_536
            || request.now_unix_secs <= 0
        {
            return Err(MemoryEffectStoreError::InvalidData(
                "memory effect identifiers, proposal, or clock are invalid".into(),
            ));
        }
        let proposal: SecretaryActionProposal = serde_json::from_str(&request.proposal_json)
            .map_err(|_| MemoryEffectStoreError::InvalidData("proposal JSON is invalid".into()))?;
        if proposal.proposal_id != request.proposal_id || proposal.action != request.action {
            return Err(MemoryEffectStoreError::InvalidData(
                "proposal identity or action does not match the effect".into(),
            ));
        }
        if !matches!(
            request.action,
            SecretaryAction::CorrectMemoryFact { .. }
                | SecretaryAction::DeleteMemoryFact { .. }
                | SecretaryAction::SetMemoryFactTtl { .. }
                | SecretaryAction::SetConversationMemoryMode { .. }
        ) {
            return Err(MemoryEffectStoreError::InvalidData(
                "action is not an Owner memory mutation".into(),
            ));
        }
        self.store.apply_owner_effect(request).await
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
