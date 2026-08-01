//! Owner 对 FollowUp 的类型化控制边界（忽略或推迟单条跟进）。
//!
//! 写操作必须由已审批的 Action Effect 触发；仓储在一个事务内复验 Action 租约、
//! OwnerCommand、账号绑定、FollowUp 来源版本，并写入业务变更、不可变审计和
//! 通用 Effect Receipt。数据库错误在 Effect 边界映射为 UnknownCommit；
//! 授权、版本冲突、租约丢失和业务状态冲突不得伪装成数据库提交不明。

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    ActionLeaseToken, ActionRunId, SecretaryAction, SecretaryActionProposal,
    SecretaryActionReceipt, SourceAccountRef, SourceEventId,
};

#[derive(Debug, Clone)]
pub struct FollowUpControlEffectRequest {
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
pub enum FollowUpControlStoreError {
    #[error("follow-up control is unauthorized")]
    Unauthorized,
    #[error("follow-up control target or state is invalid: {0}")]
    InvalidData(String),
    #[error("follow-up control lease was lost")]
    LeaseLost,
    #[error("follow-up control database operation failed")]
    Database,
}

#[async_trait]
pub trait FollowUpControlStoreT: Send + Sync {
    async fn apply_effect(
        &self,
        request: &FollowUpControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, FollowUpControlStoreError>;
}

pub struct FollowUpControlUseCase {
    store: Arc<dyn FollowUpControlStoreT>,
}

impl FollowUpControlUseCase {
    pub fn new(store: Arc<dyn FollowUpControlStoreT>) -> Self {
        Self { store }
    }

    pub async fn apply_effect(
        &self,
        request: &FollowUpControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, FollowUpControlStoreError> {
        if request.effect_id.trim().is_empty()
            || request.effect_id.len() > 255
            || request.proposal_id.trim().is_empty()
            || request.proposal_id.len() > 36
            || request.proposal_json.len() > 65_536
        {
            return Err(FollowUpControlStoreError::InvalidData(
                "follow-up control effect identifiers or proposal are invalid".into(),
            ));
        }
        let proposal: SecretaryActionProposal = serde_json::from_str(&request.proposal_json)
            .map_err(|_| {
                FollowUpControlStoreError::InvalidData(
                    "follow-up control proposal_json is invalid".into(),
                )
            })?;
        if proposal.proposal_id != request.proposal_id || proposal.action != request.action {
            return Err(FollowUpControlStoreError::InvalidData(
                "follow-up control proposal does not match the requested action".into(),
            ));
        }
        match &request.action {
            SecretaryAction::DismissFollowUp {
                expected_source_version,
                reason,
                ..
            }
            | SecretaryAction::SnoozeFollowUp {
                expected_source_version,
                reason,
                ..
            } if *expected_source_version == 0
                || reason.trim().is_empty()
                || reason.chars().count() > 1_000 =>
            {
                return Err(FollowUpControlStoreError::InvalidData(
                    "follow-up version or reason is invalid".into(),
                ));
            }
            SecretaryAction::DismissFollowUp { .. } | SecretaryAction::SnoozeFollowUp { .. } => {}
            SecretaryAction::DismissFollowUps { targets, reason } => {
                if targets.is_empty()
                    || targets.len() > 20
                    || reason.trim().is_empty()
                    || reason.chars().count() > 1_000
                {
                    return Err(FollowUpControlStoreError::InvalidData(
                        "follow-up batch targets or reason is invalid".into(),
                    ));
                }
                // 同一批次禁止重复 FollowUp ID；重复必须在进入数据库前拒绝。
                let mut seen = HashSet::new();
                for target in targets {
                    if target.expected_source_version == 0
                        || !seen.insert(target.follow_up_id.as_str())
                    {
                        return Err(FollowUpControlStoreError::InvalidData(
                            "follow-up batch targets or reason is invalid".into(),
                        ));
                    }
                }
            }
            _ => {
                return Err(FollowUpControlStoreError::InvalidData(
                    "action is not a follow-up control".into(),
                ));
            }
        }
        self.store.apply_effect(request).await
    }
}
