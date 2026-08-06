//! Owner 对失败 Artifact 派生任务的有界重处理边界。
//!
//! 该入口只能由已审批的 Action Effect 触发。基础设施实现必须在同一事务内
//! 复验账号、OwnerCommand、Action 租约与完整 proposal，并写入不可变审计和
//! 幂等 Effect Receipt。

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    ActionLeaseToken, ActionRunId, SecretaryAction, SecretaryActionProposal,
    SecretaryActionReceipt, SourceAccountRef, SourceEventId,
};

pub const MAX_ARTIFACT_REPROCESS_BATCH: u16 = 100;
pub const MAX_ARTIFACT_REPROCESS_REASON_CHARS: usize = 1_000;

#[derive(Debug, Clone)]
pub struct ArtifactReprocessEffectRequest {
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
pub enum ArtifactReprocessStoreError {
    #[error("artifact reprocess is unauthorized")]
    Unauthorized,
    #[error("artifact reprocess request or state is invalid: {0}")]
    InvalidData(String),
    #[error("artifact reprocess action lease was lost")]
    LeaseLost,
    #[error("artifact reprocess database operation failed")]
    Database,
}

#[async_trait]
pub trait ArtifactReprocessStoreT: Send + Sync {
    async fn apply_effect(
        &self,
        request: &ArtifactReprocessEffectRequest,
    ) -> Result<SecretaryActionReceipt, ArtifactReprocessStoreError>;
}

pub struct ArtifactReprocessUseCase {
    store: Arc<dyn ArtifactReprocessStoreT>,
}

impl ArtifactReprocessUseCase {
    pub fn new(store: Arc<dyn ArtifactReprocessStoreT>) -> Self {
        Self { store }
    }

    pub async fn apply_effect(
        &self,
        request: &ArtifactReprocessEffectRequest,
    ) -> Result<SecretaryActionReceipt, ArtifactReprocessStoreError> {
        if request.effect_id.trim().is_empty()
            || request.effect_id.len() > 255
            || request.proposal_id.trim().is_empty()
            || request.proposal_id.len() > 36
            || request.proposal_json.len() > 65_536
        {
            return Err(ArtifactReprocessStoreError::InvalidData(
                "artifact reprocess effect identifiers or proposal are invalid".into(),
            ));
        }
        let proposal: SecretaryActionProposal = serde_json::from_str(&request.proposal_json)
            .map_err(|_| {
                ArtifactReprocessStoreError::InvalidData(
                    "artifact reprocess proposal_json is invalid".into(),
                )
            })?;
        if proposal.proposal_id != request.proposal_id || proposal.action != request.action {
            return Err(ArtifactReprocessStoreError::InvalidData(
                "artifact reprocess proposal does not match the requested action".into(),
            ));
        }
        match &request.action {
            SecretaryAction::RetryFailedArtifactDerivations { limit, reason }
                if (1..=MAX_ARTIFACT_REPROCESS_BATCH).contains(limit)
                    && !reason.trim().is_empty()
                    && reason.chars().count() <= MAX_ARTIFACT_REPROCESS_REASON_CHARS => {}
            SecretaryAction::RetryFailedArtifactDerivations { .. } => {
                return Err(ArtifactReprocessStoreError::InvalidData(
                    "artifact reprocess limit or reason is invalid".into(),
                ));
            }
            _ => {
                return Err(ArtifactReprocessStoreError::InvalidData(
                    "action is not an artifact reprocess control".into(),
                ));
            }
        }
        self.store.apply_effect(request).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::{MessageSource, SecretaryToolKind};

    struct FakeStore {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ArtifactReprocessStoreT for FakeStore {
        async fn apply_effect(
            &self,
            request: &ArtifactReprocessEffectRequest,
        ) -> Result<SecretaryActionReceipt, ArtifactReprocessStoreError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(SecretaryActionReceipt {
                proposal_id: request.proposal_id.clone(),
                result_ref: "{\"requeued\":1}".into(),
                tool_kind: Some(SecretaryToolKind::RetryFailedArtifactDerivations),
            })
        }
    }

    fn request(limit: u16, reason: &str) -> ArtifactReprocessEffectRequest {
        let action = SecretaryAction::RetryFailedArtifactDerivations {
            limit,
            reason: reason.into(),
        };
        let proposal = SecretaryActionProposal {
            proposal_id: "proposal-1".into(),
            action: action.clone(),
            rationale: "test".into(),
            source_event_ids: vec![SourceEventId::new("event-1").unwrap()],
            idempotency_key: Some("ops004-test".into()),
        };
        ArtifactReprocessEffectRequest {
            account: SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap(),
            command_source_event_id: SourceEventId::new("event-1").unwrap(),
            run_id: ActionRunId::new("run-1").unwrap(),
            lease_token: ActionLeaseToken::generate(),
            effect_id: "effect-1".into(),
            proposal_id: proposal.proposal_id.clone(),
            proposal_json: serde_json::to_string(&proposal).unwrap(),
            action,
        }
    }

    #[tokio::test]
    async fn valid_bounded_request_reaches_store() {
        let store = Arc::new(FakeStore {
            calls: AtomicUsize::new(0),
        });
        let use_case = ArtifactReprocessUseCase::new(store.clone());
        let receipt = use_case.apply_effect(&request(100, "retry")).await.unwrap();
        assert_eq!(
            receipt.tool_kind,
            Some(SecretaryToolKind::RetryFailedArtifactDerivations)
        );
        assert_eq!(store.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn invalid_budget_or_reason_is_rejected_before_store() {
        for request in [request(0, "retry"), request(101, "retry"), request(1, "  ")] {
            let store = Arc::new(FakeStore {
                calls: AtomicUsize::new(0),
            });
            let use_case = ArtifactReprocessUseCase::new(store.clone());
            assert!(matches!(
                use_case.apply_effect(&request).await,
                Err(ArtifactReprocessStoreError::InvalidData(_))
            ));
            assert_eq!(store.calls.load(Ordering::Relaxed), 0);
        }
    }
}
