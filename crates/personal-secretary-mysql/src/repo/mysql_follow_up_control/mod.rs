//! MySqlFollowUpControlStore：Owner 对 FollowUp 的类型化控制落库。
//!
//! 模块按职责拆分，外部 builder/API 保持不变：
//! - `authorization`：授权、Effect Receipt 与稳定 control_id 派生（与
//!   ResponseExpectation 控制仓储共享，不复制授权 SQL）；
//! - `follow_up`：忽略/推迟/完成的单条与批量业务事务；
//! - `outbox`：关联通知的锁定、状态拒绝与压制；
//! - `audit`：每目标不可变审计写入。

pub(crate) mod audit;
pub(crate) mod authorization;
pub(crate) mod follow_up;
pub(crate) mod outbox;

use async_trait::async_trait;
use sea_orm::{DatabaseConnection, TransactionTrait};

use crate::{
    FollowUpControlEffectRequest, FollowUpControlStoreError, FollowUpControlStoreT,
    SecretaryAction, SecretaryActionReceipt,
};

use self::authorization::{
    ControlEffectCtx, database_error, insert_receipt_and_commit, load_receipt, lock_account,
    stable_id, verify_action_lease, verify_owner_command,
};
use self::follow_up::{
    AppliedControlBatch, apply_batch_complete, apply_batch_dismiss, apply_batch_snooze,
    apply_complete, apply_dismiss, apply_snooze,
};

pub(crate) struct MySqlFollowUpControlStore {
    db: DatabaseConnection,
}

impl MySqlFollowUpControlStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl FollowUpControlStoreT for MySqlFollowUpControlStore {
    async fn apply_effect(
        &self,
        request: &FollowUpControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, FollowUpControlStoreError> {
        let transaction = self.db.begin().await.map_err(database_error)?;
        let ctx = ControlEffectCtx::from(request);
        if let Some(receipt) = load_receipt(&transaction, &ctx).await? {
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }
        let account_id = lock_account(&transaction, &ctx).await?;
        verify_action_lease(&transaction, &ctx, account_id).await?;
        verify_owner_command(&transaction, &ctx, account_id).await?;
        // 竞争窗口内可能已有并发 Effect 写入回执；提交前再校验一次碰撞。
        if let Some(receipt) = load_receipt(&transaction, &ctx).await? {
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }

        // 单条 Dismiss/Snooze/Complete 包装为长度 1 的批次，业务行为与结果文案
        // 保持不变；批量动作走真正的批量 all-or-nothing 路径。
        let applied = match &request.action {
            SecretaryAction::DismissFollowUp { .. } => {
                let control = apply_dismiss(&transaction, request, account_id).await?;
                let result_ref = control.result_ref.clone();
                AppliedControlBatch {
                    controls: vec![control],
                    result_ref,
                }
            }
            SecretaryAction::SnoozeFollowUp { .. } => {
                let control = apply_snooze(&transaction, request, account_id).await?;
                let result_ref = control.result_ref.clone();
                AppliedControlBatch {
                    controls: vec![control],
                    result_ref,
                }
            }
            SecretaryAction::CompleteFollowUp { .. } => {
                let control = apply_complete(&transaction, request, account_id).await?;
                let result_ref = control.result_ref.clone();
                AppliedControlBatch {
                    controls: vec![control],
                    result_ref,
                }
            }
            SecretaryAction::DismissFollowUps { .. } => {
                apply_batch_dismiss(&transaction, request, account_id).await?
            }
            SecretaryAction::SnoozeFollowUps { .. } => {
                apply_batch_snooze(&transaction, request, account_id).await?
            }
            SecretaryAction::CompleteFollowUps { .. } => {
                apply_batch_complete(&transaction, request, account_id).await?
            }
            _ => {
                return Err(FollowUpControlStoreError::InvalidData(
                    "action is not a follow-up control".into(),
                ));
            }
        };
        // 单条控制沿用既有 control_id 派生（历史行为不变）；批量控制必须按
        // effect_id + follow_up_id 稳定派生（同一 Effect 每行唯一，重放不产生新 ID）。
        let is_single = matches!(
            request.action,
            SecretaryAction::DismissFollowUp { .. }
                | SecretaryAction::SnoozeFollowUp { .. }
                | SecretaryAction::CompleteFollowUp { .. }
        );
        for control in &applied.controls {
            let control_id = if is_single {
                stable_id("follow-up-control", &request.effect_id)
            } else {
                // 使用 NUL 分隔两个已分别校验的字段，避免简单冒号拼接在
                // effect_id 自身含冒号时产生边界歧义。
                stable_id(
                    "follow-up-control-batch",
                    &format!("{}\0{}", request.effect_id, control.follow_up_id),
                )
            };
            audit::insert_control_audit(&transaction, request, account_id, control, &control_id)
                .await?;
        }
        insert_receipt_and_commit(transaction, &ctx, applied.result_ref)
            .await
            .map_err(FollowUpControlStoreError::from)
    }
}
