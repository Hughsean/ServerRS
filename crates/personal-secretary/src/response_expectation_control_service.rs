//! Owner 对 ResponseExpectation 的类型化控制边界（关闭单条/批量回复期待）。
//!
//! 写操作必须由已审批的 Action Effect 触发；仓储在一个事务内复验 Action 租约、
//! OwnerCommand、账号绑定、期望来源版本，并写入业务变更、不可变审计和
//! 通用 Effect Receipt。授权、Receipt 与稳定 ID 逻辑与 FollowUp 控制共享，
//! 不复制稍有不同的授权 SQL。数据库错误在 Effect 边界映射为 UnknownCommit；
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
pub struct ResponseExpectationControlEffectRequest {
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
pub enum ResponseExpectationControlStoreError {
    #[error("response expectation control is unauthorized")]
    Unauthorized,
    #[error("response expectation control target or state is invalid: {0}")]
    InvalidData(String),
    #[error("response expectation control lease was lost")]
    LeaseLost,
    #[error("response expectation control database operation failed")]
    Database,
}

#[async_trait]
pub trait ResponseExpectationControlStoreT: Send + Sync {
    async fn apply_effect(
        &self,
        request: &ResponseExpectationControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, ResponseExpectationControlStoreError>;
}

pub struct ResponseExpectationControlUseCase {
    store: Arc<dyn ResponseExpectationControlStoreT>,
}

impl ResponseExpectationControlUseCase {
    pub fn new(store: Arc<dyn ResponseExpectationControlStoreT>) -> Self {
        Self { store }
    }

    pub async fn apply_effect(
        &self,
        request: &ResponseExpectationControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, ResponseExpectationControlStoreError> {
        if request.effect_id.trim().is_empty()
            || request.effect_id.len() > 255
            || request.proposal_id.trim().is_empty()
            || request.proposal_id.len() > 36
            || request.proposal_json.len() > 65_536
        {
            return Err(ResponseExpectationControlStoreError::InvalidData(
                "response expectation control effect identifiers or proposal are invalid".into(),
            ));
        }
        let proposal: SecretaryActionProposal = serde_json::from_str(&request.proposal_json)
            .map_err(|_| {
                ResponseExpectationControlStoreError::InvalidData(
                    "response expectation control proposal_json is invalid".into(),
                )
            })?;
        if proposal.proposal_id != request.proposal_id || proposal.action != request.action {
            return Err(ResponseExpectationControlStoreError::InvalidData(
                "response expectation control proposal does not match the requested action".into(),
            ));
        }
        match &request.action {
            SecretaryAction::DismissResponseExpectation {
                expected_source_version,
                reason,
                ..
            } => {
                if *expected_source_version == 0
                    || reason.trim().is_empty()
                    || reason.chars().count() > 1_000
                {
                    return Err(ResponseExpectationControlStoreError::InvalidData(
                        "response expectation version or reason is invalid".into(),
                    ));
                }
            }
            SecretaryAction::DismissResponseExpectations { targets, reason } => {
                validate_expectation_batch_targets(targets, reason)?;
            }
            _ => {
                return Err(ResponseExpectationControlStoreError::InvalidData(
                    "action is not a response expectation control".into(),
                ));
            }
        }
        self.store.apply_effect(request).await
    }
}

/// 批量关闭回复期待共用的目标与 reason 校验：1..=20、ID 不重复、
/// 版本为正、reason 去除首尾空白后 1..=1000 字符；重复必须在进入数据库前拒绝。
fn validate_expectation_batch_targets(
    targets: &[crate::ResponseExpectationControlTarget],
    reason: &str,
) -> Result<(), ResponseExpectationControlStoreError> {
    if targets.is_empty()
        || targets.len() > 20
        || reason.trim().is_empty()
        || reason.chars().count() > 1_000
    {
        return Err(ResponseExpectationControlStoreError::InvalidData(
            "response expectation batch targets or reason is invalid".into(),
        ));
    }
    let mut seen = HashSet::new();
    for target in targets {
        if target.expected_source_version == 0 || !seen.insert(target.expectation_id.as_str()) {
            return Err(ResponseExpectationControlStoreError::InvalidData(
                "response expectation batch targets or reason is invalid".into(),
            ));
        }
    }
    Ok(())
}
