//! MySqlResponseExpectationControlStore：Owner 对回复期待的关闭控制落库。
//!
//! 授权、Effect Receipt 与稳定 control_id 派生复用
//! `mysql_follow_up_control::authorization`，不复制授权 SQL；业务事务
//! （active -> dismissed、版本精确 +1、due 不变）、Candidate 回溯关联与
//! 本模块审计表写入是本模块自己的状态机。

use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};

use crate::{
    ResponseExpectationControlEffectRequest, ResponseExpectationControlStoreError,
    ResponseExpectationControlStoreT, ResponseExpectationControlTarget, SecretaryAction,
    SecretaryActionReceipt,
};

use super::mysql_follow_up_control::authorization::{
    ControlEffectCtx, database_error, insert_receipt_and_commit, load_receipt, lock_account,
    stable_id, verify_action_lease, verify_owner_command,
};
use super::mysql_follow_up_control::outbox::{lock_and_check_outbox, suppress_pending_outbox};

pub(crate) struct MySqlResponseExpectationControlStore {
    db: DatabaseConnection,
}

impl MySqlResponseExpectationControlStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ResponseExpectationControlStoreT for MySqlResponseExpectationControlStore {
    async fn apply_effect(
        &self,
        request: &ResponseExpectationControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, ResponseExpectationControlStoreError> {
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

        // 单条 DismissResponseExpectation 包装为长度 1 的批次；
        // 批量关闭走真正的批量 all-or-nothing 路径。
        let applied = match &request.action {
            SecretaryAction::DismissResponseExpectation { .. } => {
                let control = apply_dismiss(&transaction, request, account_id).await?;
                let result_ref = control.result_ref.clone();
                AppliedExpectationControlBatch {
                    controls: vec![control],
                    result_ref,
                }
            }
            SecretaryAction::DismissResponseExpectations { .. } => {
                apply_batch_dismiss(&transaction, request, account_id).await?
            }
            _ => {
                return Err(ResponseExpectationControlStoreError::InvalidData(
                    "action is not a response expectation control".into(),
                ));
            }
        };
        // 单条控制沿用单条派生；批量控制必须按 effect_id + expectation_id 稳定
        // 派生（同一 Effect 每行唯一，重放不产生新 ID），同样使用 NUL 分隔两个
        // 已分别校验的字段，避免简单冒号拼接在 effect_id 含冒号时产生边界歧义。
        let is_single = matches!(
            request.action,
            SecretaryAction::DismissResponseExpectation { .. }
        );
        for control in &applied.controls {
            let control_id = if is_single {
                stable_id("response-expectation-control", &request.effect_id)
            } else {
                stable_id(
                    "response-expectation-control-batch",
                    &format!("{}\0{}", request.effect_id, control.expectation_id),
                )
            };
            insert_control_audit(&transaction, request, account_id, control, &control_id).await?;
        }
        insert_receipt_and_commit(transaction, &ctx, applied.result_ref)
            .await
            .map_err(ResponseExpectationControlStoreError::from)
    }
}

/// 关闭动作的落库结果；dismiss 单条/批量共用审计与回执写入。
struct AppliedExpectationControl {
    expectation_id: String,
    previous_status: &'static str,
    current_status: &'static str,
    previous_source_version: u64,
    current_source_version: u64,
    reason: String,
    result_ref: String,
}

/// 一次 Effect 的完整落库结果：单条包装为长度 1 的批次，批量包含多个目标；
/// 整个批次只写一条通用 Effect Receipt。
struct AppliedExpectationControlBatch {
    controls: Vec<AppliedExpectationControl>,
    result_ref: String,
}

/// 锁定回复期待并执行关闭：active -> dismissed，版本精确 +1，due 不变。
/// ResponseExpectation 只经 policy-owned Candidate 回溯关联通知
/// （legacy outbox.follow_up_id 直连不适用）。
async fn apply_dismiss<C: ConnectionTrait>(
    db: &C,
    request: &ResponseExpectationControlEffectRequest,
    account_id: u64,
) -> Result<AppliedExpectationControl, ResponseExpectationControlStoreError> {
    let (expectation_id, expected_source_version, reason) = match &request.action {
        SecretaryAction::DismissResponseExpectation {
            expectation_id,
            expected_source_version,
            reason,
        } => (
            expectation_id.as_str(),
            *expected_source_version,
            reason.clone(),
        ),
        _ => {
            return Err(ResponseExpectationControlStoreError::InvalidData(
                "action is not a response expectation dismiss".into(),
            ));
        }
    };
    let item = ExpectationRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT expectation_status, source_version FROM secretary_response_expectations \
         WHERE expectation_id = ? AND account_id = ? FOR UPDATE",
        [expectation_id.into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or_else(|| {
        ResponseExpectationControlStoreError::InvalidData(
            "response expectation not found in account".into(),
        )
    })?;
    if item.expectation_status != "active" || item.source_version != expected_source_version {
        return Err(ResponseExpectationControlStoreError::InvalidData(
            "response expectation status or source_version changed since approval".into(),
        ));
    }
    lock_and_check_outbox(
        db,
        account_id,
        None,
        "response_expectation",
        expectation_id,
        "dismiss",
    )
    .await?;
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_response_expectations \
             SET expectation_status = 'dismissed', source_version = source_version + 1, \
                 updated_at = CURRENT_TIMESTAMP(6) \
             WHERE expectation_id = ? AND account_id = ? AND expectation_status = 'active' \
               AND source_version = ?",
            [
                expectation_id.into(),
                account_id.into(),
                expected_source_version.into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if updated.rows_affected() != 1 {
        return Err(ResponseExpectationControlStoreError::InvalidData(
            "response expectation compare-and-set failed".into(),
        ));
    }
    suppress_pending_outbox(db, account_id, None, "response_expectation", expectation_id).await?;
    Ok(AppliedExpectationControl {
        expectation_id: expectation_id.to_owned(),
        previous_status: "active",
        current_status: "dismissed",
        previous_source_version: item.source_version,
        current_source_version: item.source_version + 1,
        reason,
        result_ref: format!(
            "回复期待 {} 已关闭（版本 {} -> {}）",
            expectation_id,
            item.source_version,
            item.source_version + 1
        ),
    })
}

/// 批量关闭：按 expectation_id 字典序锁定（确定性锁顺序，避免重叠批次死锁），
/// 先验证全部目标与关联 Outbox 再执行任何业务 UPDATE；任一失败整个事务回滚，
/// 不允许部分成功（all-or-nothing）。
async fn apply_batch_dismiss<C: ConnectionTrait>(
    db: &C,
    request: &ResponseExpectationControlEffectRequest,
    account_id: u64,
) -> Result<AppliedExpectationControlBatch, ResponseExpectationControlStoreError> {
    let (targets, reason) = match &request.action {
        SecretaryAction::DismissResponseExpectations { targets, reason } => {
            (targets, reason.clone())
        }
        _ => {
            return Err(ResponseExpectationControlStoreError::InvalidData(
                "action is not a batch response expectation dismiss".into(),
            ));
        }
    };
    // 确定性锁顺序：不能按 LLM 给出的原始顺序直接锁库。
    let mut ordered: Vec<&ResponseExpectationControlTarget> = targets.iter().collect();
    ordered.sort_by(|a, b| a.expectation_id.as_str().cmp(b.expectation_id.as_str()));

    // 阶段 1：锁定全部目标并校验账号/状态/版本；任一不存在/跨账号/不匹配立即失败。
    let mut locked = Vec::with_capacity(ordered.len());
    for target in &ordered {
        let item = ExpectationRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT expectation_status, source_version FROM secretary_response_expectations \
             WHERE expectation_id = ? AND account_id = ? FOR UPDATE",
            [target.expectation_id.as_str().into(), account_id.into()],
        ))
        .one(db)
        .await
        .map_err(database_error)?
        .ok_or_else(|| {
            ResponseExpectationControlStoreError::InvalidData(
                "response expectation not found in account".into(),
            )
        })?;
        if item.expectation_status != "active"
            || item.source_version != target.expected_source_version
        {
            return Err(ResponseExpectationControlStoreError::InvalidData(
                "response expectation status or source_version changed since approval".into(),
            ));
        }
        locked.push((target, item));
    }
    // 阶段 2：锁定全部关联 Outbox（policy-owned Candidate 回溯）并拒绝 claimed。
    for target in &ordered {
        lock_and_check_outbox(
            db,
            account_id,
            None,
            "response_expectation",
            target.expectation_id.as_str(),
            "dismiss",
        )
        .await?;
    }
    // 阶段 3：全部校验通过后才执行 CAS 更新（active -> dismissed，version 精确 +1）。
    for (target, _) in &locked {
        let updated = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_response_expectations \
                 SET expectation_status = 'dismissed', source_version = source_version + 1, \
                     updated_at = CURRENT_TIMESTAMP(6) \
                 WHERE expectation_id = ? AND account_id = ? AND expectation_status = 'active' \
                   AND source_version = ?",
                [
                    target.expectation_id.as_str().into(),
                    account_id.into(),
                    target.expected_source_version.into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        if updated.rows_affected() != 1 {
            return Err(ResponseExpectationControlStoreError::InvalidData(
                "response expectation compare-and-set failed".into(),
            ));
        }
    }
    // 阶段 4：压制全部目标的 pending/failed Outbox 并清除租约；delivered 保留。
    for target in &ordered {
        suppress_pending_outbox(
            db,
            account_id,
            None,
            "response_expectation",
            target.expectation_id.as_str(),
        )
        .await?;
    }
    // 阶段 5：组装每目标审计与有界结果文案（只含数量与 expectation ID）。
    let mut controls = Vec::with_capacity(locked.len());
    for (target, item) in &locked {
        controls.push(AppliedExpectationControl {
            expectation_id: target.expectation_id.as_str().to_owned(),
            previous_status: "active",
            current_status: "dismissed",
            previous_source_version: item.source_version,
            current_source_version: item.source_version + 1,
            reason: reason.clone(),
            result_ref: format!(
                "回复期待 {} 已关闭（版本 {} -> {}）",
                target.expectation_id.as_str(),
                item.source_version,
                item.source_version + 1
            ),
        });
    }
    // 20 个目标 × 37 字符仍在响应有界约束内；不包含账号 ID/OpenID/Token/聊天正文。
    let ids = controls
        .iter()
        .map(|control| control.expectation_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let result_ref = format!("已批量关闭 {} 条回复期待：{ids}", controls.len());
    Ok(AppliedExpectationControlBatch {
        controls,
        result_ref,
    })
}

/// 每目标一行不可变审计；`(effect_id, expectation_id)` 复合唯一键保证重放不重复。
async fn insert_control_audit<C: ConnectionTrait>(
    db: &C,
    request: &ResponseExpectationControlEffectRequest,
    account_id: u64,
    applied: &AppliedExpectationControl,
    control_id: &str,
) -> Result<(), ResponseExpectationControlStoreError> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_response_expectation_owner_controls \
             (control_id, effect_id, run_id, proposal_id, account_id, expectation_id, \
              previous_status, current_status, previous_source_version, current_source_version, \
              command_source_event_id, reason) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            [
                control_id.into(),
                request.effect_id.clone().into(),
                request.run_id.as_str().into(),
                request.proposal_id.clone().into(),
                account_id.into(),
                applied.expectation_id.clone().into(),
                applied.previous_status.into(),
                applied.current_status.into(),
                applied.previous_source_version.into(),
                applied.current_source_version.into(),
                request.command_source_event_id.as_str().into(),
                applied.reason.clone().into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err(ResponseExpectationControlStoreError::Database);
    }
    Ok(())
}

#[derive(FromQueryResult)]
struct ExpectationRow {
    expectation_status: String,
    source_version: u64,
}
