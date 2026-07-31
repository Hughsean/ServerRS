//! Owner 对线程语义和生命周期的类型化控制边界。
//!
//! 写操作必须由已审批的 Action Effect 触发；仓储在一个事务内复验 Action 租约、
//! OwnerCommand、账号绑定、目标账号并写入业务变更、审计和 Effect Receipt。

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    ActionLeaseToken, ActionRunId, SecretaryAction, SecretaryActionReceipt, SourceAccountRef,
    SourceEventId,
};

#[derive(Debug, Clone)]
pub struct ThreadControlEffectRequest {
    pub account: SourceAccountRef,
    pub command_source_event_id: SourceEventId,
    pub run_id: ActionRunId,
    pub lease_token: ActionLeaseToken,
    pub effect_id: String,
    pub proposal_id: String,
    pub proposal_json: String,
    pub action: SecretaryAction,
}

#[derive(Debug, Error)]
pub enum ThreadControlStoreError {
    #[error("thread control is unauthorized")]
    Unauthorized,
    #[error("thread control target or state is invalid: {0}")]
    InvalidData(String),
    #[error("thread control lease was lost")]
    LeaseLost,
    #[error("thread control database operation failed")]
    Database,
}

#[async_trait]
pub trait ThreadControlStoreT: Send + Sync {
    async fn apply_effect(
        &self,
        request: &ThreadControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, ThreadControlStoreError>;
}

pub struct ThreadControlUseCase {
    store: Arc<dyn ThreadControlStoreT>,
}

impl ThreadControlUseCase {
    pub fn new(store: Arc<dyn ThreadControlStoreT>) -> Self {
        Self { store }
    }

    pub async fn apply_effect(
        &self,
        request: &ThreadControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, ThreadControlStoreError> {
        if request.effect_id.trim().is_empty()
            || request.effect_id.len() > 255
            || request.proposal_id.trim().is_empty()
            || request.proposal_json.len() > 65_536
        {
            return Err(ThreadControlStoreError::InvalidData(
                "thread control effect identifiers or proposal are invalid".into(),
            ));
        }
        if !matches!(
            request.action,
            SecretaryAction::ConfirmThreadDecision { .. }
                | SecretaryAction::RevokeThreadDecision { .. }
                | SecretaryAction::DismissThreadQuestion { .. }
                | SecretaryAction::SetThreadLifecycle { .. }
        ) {
            return Err(ThreadControlStoreError::InvalidData(
                "action is not a thread control".into(),
            ));
        }
        self.store.apply_effect(request).await
    }
}
