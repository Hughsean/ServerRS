//! FollowUp 控制的业务事务：单条/批量忽略、推迟与完成。
//!
//! 所有路径共用：目标按 follow_up_id 字典序锁定（确定性锁顺序，避免重叠批次
//! 死锁）、先验证全部目标与关联 Outbox 再执行任何业务 UPDATE（all-or-nothing）、
//! CAS 更新、压制 pending/failed Outbox。审计与整批一条 Effect Receipt 由
//! mod.rs 统一写入。

use sea_orm::{ConnectionTrait, DatabaseBackend, FromQueryResult, Statement};

use crate::{
    FollowUpControlEffectRequest, FollowUpControlStoreError, FollowUpControlTarget, SecretaryAction,
};

use super::authorization::database_error;
use super::outbox::{lock_and_check_outbox, suppress_pending_outbox};

/// 各 FollowUp 控制动作的统一落库结果；dismiss/snooze/complete 共用审计与回执写入。
pub(super) struct AppliedControl {
    pub follow_up_id: String,
    pub control_kind: &'static str,
    pub previous_status: &'static str,
    pub current_status: &'static str,
    pub previous_source_version: u64,
    pub current_source_version: u64,
    pub previous_due_at_unix_secs: Option<i64>,
    pub current_due_at_unix_secs: Option<i64>,
    pub reason: String,
    pub result_ref: String,
}

/// 一次 Effect 的完整落库结果：单条动作包装为长度 1 的批次，
/// 批量动作为多个目标；整个批次只写一条通用 Effect Receipt。
pub(super) struct AppliedControlBatch {
    pub controls: Vec<AppliedControl>,
    pub result_ref: String,
}

/// 锁定 FollowUp 并执行忽略：状态/版本 CAS、Outbox 状态拒绝与压制。
pub(super) async fn apply_dismiss<C: ConnectionTrait>(
    db: &C,
    request: &FollowUpControlEffectRequest,
    account_id: u64,
) -> Result<AppliedControl, FollowUpControlStoreError> {
    let (follow_up_id, expected_source_version, reason) = match &request.action {
        SecretaryAction::DismissFollowUp {
            follow_up_id,
            expected_source_version,
            reason,
        } => (
            follow_up_id.as_str(),
            *expected_source_version,
            reason.clone(),
        ),
        _ => {
            return Err(FollowUpControlStoreError::InvalidData(
                "action is not a follow-up dismiss".into(),
            ));
        }
    };
    let item = FollowUpItemRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT status, due_at_unix_secs, source_version FROM secretary_follow_up_items \
         WHERE follow_up_id = ? AND account_id = ? FOR UPDATE",
        [follow_up_id.into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or_else(|| {
        FollowUpControlStoreError::InvalidData("follow_up not found in account".into())
    })?;
    if item.status != "scheduled" || item.source_version != expected_source_version {
        return Err(FollowUpControlStoreError::InvalidData(
            "follow_up status or source_version changed since approval".into(),
        ));
    }
    lock_and_check_outbox(
        db,
        account_id,
        Some(follow_up_id),
        "follow_up",
        follow_up_id,
        "dismiss",
    )
    .await?;
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_follow_up_items \
             SET status = 'dismissed', source_version = source_version + 1, \
                 updated_at = CURRENT_TIMESTAMP(6) \
             WHERE follow_up_id = ? AND account_id = ? AND status = 'scheduled' \
               AND source_version = ?",
            [
                follow_up_id.into(),
                account_id.into(),
                expected_source_version.into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if updated.rows_affected() != 1 {
        return Err(FollowUpControlStoreError::InvalidData(
            "follow-up compare-and-set failed".into(),
        ));
    }
    suppress_pending_outbox(
        db,
        account_id,
        Some(follow_up_id),
        "follow_up",
        follow_up_id,
    )
    .await?;
    Ok(AppliedControl {
        follow_up_id: follow_up_id.to_owned(),
        control_kind: "dismiss",
        previous_status: "scheduled",
        current_status: "dismissed",
        previous_source_version: item.source_version,
        current_source_version: item.source_version + 1,
        previous_due_at_unix_secs: None,
        current_due_at_unix_secs: None,
        reason,
        result_ref: format!(
            "跟进事项 {} 已忽略（版本 {} -> {}）",
            follow_up_id,
            item.source_version,
            item.source_version + 1
        ),
    })
}

/// 锁定 FollowUp 并执行推迟：时间窗口、状态/版本 CAS、Outbox 状态拒绝与压制。
pub(super) async fn apply_snooze<C: ConnectionTrait>(
    db: &C,
    request: &FollowUpControlEffectRequest,
    account_id: u64,
) -> Result<AppliedControl, FollowUpControlStoreError> {
    let (follow_up_id, expected_source_version, snooze_until_unix_secs, reason) =
        match &request.action {
            SecretaryAction::SnoozeFollowUp {
                follow_up_id,
                expected_source_version,
                snooze_until_unix_secs,
                reason,
            } => (
                follow_up_id.as_str(),
                *expected_source_version,
                *snooze_until_unix_secs,
                reason.clone(),
            ),
            _ => {
                return Err(FollowUpControlStoreError::InvalidData(
                    "action is not a follow-up snooze".into(),
                ));
            }
        };
    let item = FollowUpItemRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT status, due_at_unix_secs, source_version FROM secretary_follow_up_items \
         WHERE follow_up_id = ? AND account_id = ? FOR UPDATE",
        [follow_up_id.into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or_else(|| {
        FollowUpControlStoreError::InvalidData("follow_up not found in account".into())
    })?;
    if item.status != "scheduled" || item.source_version != expected_source_version {
        return Err(FollowUpControlStoreError::InvalidData(
            "follow_up status or source_version changed since approval".into(),
        ));
    }
    // 时间校验以数据库当前 UTC 时间为准，不能只相信 Planner 或审批前时间；
    // 365 天 = 31_536_000 秒。
    let window = TimeWindowRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT (? > UNIX_TIMESTAMP()) AS is_future, \
                (? <= UNIX_TIMESTAMP() + ?) AS within_365_days \
         FROM DUAL",
        [
            snooze_until_unix_secs.into(),
            snooze_until_unix_secs.into(),
            31_536_000i64.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or(FollowUpControlStoreError::Database)?;
    if window.is_future == 0 {
        return Err(FollowUpControlStoreError::InvalidData(
            "follow_up snooze_until must be later than the database current time".into(),
        ));
    }
    if window.within_365_days == 0 {
        return Err(FollowUpControlStoreError::InvalidData(
            "follow_up snooze_until must be within 365 days of the database current time".into(),
        ));
    }
    if snooze_until_unix_secs <= item.due_at_unix_secs {
        return Err(FollowUpControlStoreError::InvalidData(
            "follow_up snooze_until must be later than the current due time".into(),
        ));
    }
    lock_and_check_outbox(
        db,
        account_id,
        Some(follow_up_id),
        "follow_up",
        follow_up_id,
        "snooze",
    )
    .await?;
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_follow_up_items \
             SET due_at_unix_secs = ?, source_version = source_version + 1, \
                 updated_at = CURRENT_TIMESTAMP(6) \
             WHERE follow_up_id = ? AND account_id = ? AND status = 'scheduled' \
               AND source_version = ?",
            [
                snooze_until_unix_secs.into(),
                follow_up_id.into(),
                account_id.into(),
                expected_source_version.into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if updated.rows_affected() != 1 {
        return Err(FollowUpControlStoreError::InvalidData(
            "follow-up compare-and-set failed".into(),
        ));
    }
    suppress_pending_outbox(
        db,
        account_id,
        Some(follow_up_id),
        "follow_up",
        follow_up_id,
    )
    .await?;
    Ok(AppliedControl {
        follow_up_id: follow_up_id.to_owned(),
        control_kind: "snooze",
        previous_status: "scheduled",
        current_status: "scheduled",
        previous_source_version: item.source_version,
        current_source_version: item.source_version + 1,
        previous_due_at_unix_secs: Some(item.due_at_unix_secs),
        current_due_at_unix_secs: Some(snooze_until_unix_secs),
        reason,
        result_ref: format!(
            "跟进事项 {} 已推迟到 {}（版本 {} -> {}）",
            follow_up_id,
            snooze_until_unix_secs,
            item.source_version,
            item.source_version + 1
        ),
    })
}

/// 批量忽略：按 follow_up_id 字典序锁定（确定性锁顺序，避免重叠批次死锁），
/// 先验证全部目标与关联 Outbox 再执行任何业务 UPDATE；任一失败整个事务回滚，
/// 不允许部分成功（all-or-nothing）。
pub(super) async fn apply_batch_dismiss<C: ConnectionTrait>(
    db: &C,
    request: &FollowUpControlEffectRequest,
    account_id: u64,
) -> Result<AppliedControlBatch, FollowUpControlStoreError> {
    let (targets, reason) = match &request.action {
        SecretaryAction::DismissFollowUps { targets, reason } => (targets, reason.clone()),
        _ => {
            return Err(FollowUpControlStoreError::InvalidData(
                "action is not a batch follow-up dismiss".into(),
            ));
        }
    };
    // 确定性锁顺序：不能按 LLM 给出的原始顺序直接锁库。
    let mut ordered: Vec<&FollowUpControlTarget> = targets.iter().collect();
    ordered.sort_by(|a, b| a.follow_up_id.as_str().cmp(b.follow_up_id.as_str()));

    // 阶段 1：锁定全部目标并校验状态/版本；任一不存在/不匹配立即失败。
    let mut locked = Vec::with_capacity(ordered.len());
    for target in &ordered {
        let item = FollowUpItemRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT status, due_at_unix_secs, source_version FROM secretary_follow_up_items \
             WHERE follow_up_id = ? AND account_id = ? FOR UPDATE",
            [target.follow_up_id.as_str().into(), account_id.into()],
        ))
        .one(db)
        .await
        .map_err(database_error)?
        .ok_or_else(|| {
            FollowUpControlStoreError::InvalidData("follow_up not found in account".into())
        })?;
        if item.status != "scheduled" || item.source_version != target.expected_source_version {
            return Err(FollowUpControlStoreError::InvalidData(
                "follow_up status or source_version changed since approval".into(),
            ));
        }
        locked.push((target, item));
    }
    // 阶段 2：锁定全部关联 Outbox（legacy + policy-owned 回溯）并拒绝 claimed。
    for target in &ordered {
        lock_and_check_outbox(
            db,
            account_id,
            Some(target.follow_up_id.as_str()),
            "follow_up",
            target.follow_up_id.as_str(),
            "dismiss",
        )
        .await?;
    }
    // 阶段 3：全部校验通过后才执行 CAS 更新（status -> dismissed，version 精确 +1）。
    for (target, _) in &locked {
        let updated = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_follow_up_items \
                 SET status = 'dismissed', source_version = source_version + 1, \
                     updated_at = CURRENT_TIMESTAMP(6) \
                 WHERE follow_up_id = ? AND account_id = ? AND status = 'scheduled' \
                   AND source_version = ?",
                [
                    target.follow_up_id.as_str().into(),
                    account_id.into(),
                    target.expected_source_version.into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        if updated.rows_affected() != 1 {
            return Err(FollowUpControlStoreError::InvalidData(
                "follow-up compare-and-set failed".into(),
            ));
        }
    }
    // 阶段 4：压制全部目标的 pending/failed Outbox 并清除租约；delivered 保留。
    for target in &ordered {
        suppress_pending_outbox(
            db,
            account_id,
            Some(target.follow_up_id.as_str()),
            "follow_up",
            target.follow_up_id.as_str(),
        )
        .await?;
    }
    // 阶段 5：组装每目标审计与有界结果文案（只含数量与 FollowUp ID）。
    let mut controls = Vec::with_capacity(locked.len());
    for (target, item) in &locked {
        controls.push(AppliedControl {
            follow_up_id: target.follow_up_id.as_str().to_owned(),
            control_kind: "dismiss",
            previous_status: "scheduled",
            current_status: "dismissed",
            previous_source_version: item.source_version,
            current_source_version: item.source_version + 1,
            previous_due_at_unix_secs: None,
            current_due_at_unix_secs: None,
            reason: reason.clone(),
            result_ref: format!(
                "跟进事项 {} 已忽略（版本 {} -> {}）",
                target.follow_up_id.as_str(),
                item.source_version,
                item.source_version + 1
            ),
        });
    }
    // 20 个目标 × 37 字符仍在响应有界约束内；不包含账号 ID/OpenID/Token/聊天正文。
    let ids = controls
        .iter()
        .map(|control| control.follow_up_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let result_ref = format!("已批量忽略 {} 条跟进事项：{ids}", controls.len());
    Ok(AppliedControlBatch {
        controls,
        result_ref,
    })
}

/// 批量推迟：按 follow_up_id 字典序锁定（确定性锁顺序，避免重叠批次死锁），
/// 先验证全部目标、共同新时间与关联 Outbox 再执行任何业务 UPDATE；任一失败
/// 整个事务回滚，不允许部分成功（all-or-nothing）。
pub(super) async fn apply_batch_snooze<C: ConnectionTrait>(
    db: &C,
    request: &FollowUpControlEffectRequest,
    account_id: u64,
) -> Result<AppliedControlBatch, FollowUpControlStoreError> {
    let (targets, snooze_until_unix_secs, reason) = match &request.action {
        SecretaryAction::SnoozeFollowUps {
            targets,
            snooze_until_unix_secs,
            reason,
        } => (targets, *snooze_until_unix_secs, reason.clone()),
        _ => {
            return Err(FollowUpControlStoreError::InvalidData(
                "action is not a batch follow-up snooze".into(),
            ));
        }
    };
    // 确定性锁顺序：不能按 LLM 给出的原始顺序直接锁库。
    let mut ordered: Vec<&FollowUpControlTarget> = targets.iter().collect();
    ordered.sort_by(|a, b| a.follow_up_id.as_str().cmp(b.follow_up_id.as_str()));

    // 阶段 1：锁定全部目标并校验状态/版本；任一不存在/不匹配立即失败。
    let mut locked = Vec::with_capacity(ordered.len());
    for target in &ordered {
        let item = FollowUpItemRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT status, due_at_unix_secs, source_version FROM secretary_follow_up_items \
             WHERE follow_up_id = ? AND account_id = ? FOR UPDATE",
            [target.follow_up_id.as_str().into(), account_id.into()],
        ))
        .one(db)
        .await
        .map_err(database_error)?
        .ok_or_else(|| {
            FollowUpControlStoreError::InvalidData("follow_up not found in account".into())
        })?;
        if item.status != "scheduled" || item.source_version != target.expected_source_version {
            return Err(FollowUpControlStoreError::InvalidData(
                "follow_up status or source_version changed since approval".into(),
            ));
        }
        locked.push((target, item));
    }
    // 阶段 2：以数据库当前 UTC 时间统一校验共同新时间（整批一次判定）；
    // 不能只相信 Planner 或审批前时间。365 天 = 31_536_000 秒。
    let window = TimeWindowRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT (? > UNIX_TIMESTAMP()) AS is_future, \
                (? <= UNIX_TIMESTAMP() + ?) AS within_365_days \
         FROM DUAL",
        [
            snooze_until_unix_secs.into(),
            snooze_until_unix_secs.into(),
            31_536_000i64.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or(FollowUpControlStoreError::Database)?;
    if window.is_future == 0 {
        return Err(FollowUpControlStoreError::InvalidData(
            "follow_up snooze_until must be later than the database current time".into(),
        ));
    }
    if window.within_365_days == 0 {
        return Err(FollowUpControlStoreError::InvalidData(
            "follow_up snooze_until must be within 365 days of the database current time".into(),
        ));
    }
    for (_, item) in &locked {
        if snooze_until_unix_secs <= item.due_at_unix_secs {
            return Err(FollowUpControlStoreError::InvalidData(
                "follow_up snooze_until must be later than every target's current due time".into(),
            ));
        }
    }
    // 阶段 3：锁定全部关联 Outbox（legacy + policy-owned 回溯）并拒绝 claimed。
    for target in &ordered {
        lock_and_check_outbox(
            db,
            account_id,
            Some(target.follow_up_id.as_str()),
            "follow_up",
            target.follow_up_id.as_str(),
            "snooze",
        )
        .await?;
    }
    // 阶段 4：全部校验通过后才执行 CAS 更新（due -> 共同新时间，version 精确 +1）。
    for (target, _) in &locked {
        let updated = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_follow_up_items \
                 SET due_at_unix_secs = ?, source_version = source_version + 1, \
                     updated_at = CURRENT_TIMESTAMP(6) \
                 WHERE follow_up_id = ? AND account_id = ? AND status = 'scheduled' \
                   AND source_version = ?",
                [
                    snooze_until_unix_secs.into(),
                    target.follow_up_id.as_str().into(),
                    account_id.into(),
                    target.expected_source_version.into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        if updated.rows_affected() != 1 {
            return Err(FollowUpControlStoreError::InvalidData(
                "follow-up compare-and-set failed".into(),
            ));
        }
    }
    // 阶段 5：压制全部目标的 pending/failed Outbox 并清除租约；delivered 保留。
    for target in &ordered {
        suppress_pending_outbox(
            db,
            account_id,
            Some(target.follow_up_id.as_str()),
            "follow_up",
            target.follow_up_id.as_str(),
        )
        .await?;
    }
    // 阶段 6：组装每目标审计与有界结果文案（只含数量、FollowUp ID 与共同新时间）。
    let mut controls = Vec::with_capacity(locked.len());
    for (target, item) in &locked {
        controls.push(AppliedControl {
            follow_up_id: target.follow_up_id.as_str().to_owned(),
            control_kind: "snooze",
            previous_status: "scheduled",
            current_status: "scheduled",
            previous_source_version: item.source_version,
            current_source_version: item.source_version + 1,
            previous_due_at_unix_secs: Some(item.due_at_unix_secs),
            current_due_at_unix_secs: Some(snooze_until_unix_secs),
            reason: reason.clone(),
            result_ref: format!(
                "跟进事项 {} 已推迟到 {}（版本 {} -> {}）",
                target.follow_up_id.as_str(),
                snooze_until_unix_secs,
                item.source_version,
                item.source_version + 1
            ),
        });
    }
    // 20 个目标 × 37 字符仍在响应有界约束内；不包含账号 ID/OpenID/Token/聊天正文。
    let ids = controls
        .iter()
        .map(|control| control.follow_up_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let result_ref = format!(
        "已批量推迟 {} 条跟进事项到 {}：{ids}",
        controls.len(),
        snooze_until_unix_secs
    );
    Ok(AppliedControlBatch {
        controls,
        result_ref,
    })
}

/// 锁定 FollowUp 并执行完成：scheduled -> completed，版本精确 +1，due 不变；
/// 完成后关联通知被压制，Scheduler 不得重新创建该事项。
pub(super) async fn apply_complete<C: ConnectionTrait>(
    db: &C,
    request: &FollowUpControlEffectRequest,
    account_id: u64,
) -> Result<AppliedControl, FollowUpControlStoreError> {
    let (follow_up_id, expected_source_version, reason) = match &request.action {
        SecretaryAction::CompleteFollowUp {
            follow_up_id,
            expected_source_version,
            reason,
        } => (
            follow_up_id.as_str(),
            *expected_source_version,
            reason.clone(),
        ),
        _ => {
            return Err(FollowUpControlStoreError::InvalidData(
                "action is not a follow-up complete".into(),
            ));
        }
    };
    let item = FollowUpItemRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT status, due_at_unix_secs, source_version FROM secretary_follow_up_items \
         WHERE follow_up_id = ? AND account_id = ? FOR UPDATE",
        [follow_up_id.into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or_else(|| {
        FollowUpControlStoreError::InvalidData("follow_up not found in account".into())
    })?;
    if item.status != "scheduled" || item.source_version != expected_source_version {
        return Err(FollowUpControlStoreError::InvalidData(
            "follow_up status or source_version changed since approval".into(),
        ));
    }
    lock_and_check_outbox(
        db,
        account_id,
        Some(follow_up_id),
        "follow_up",
        follow_up_id,
        "complete",
    )
    .await?;
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_follow_up_items \
             SET status = 'completed', source_version = source_version + 1, \
                 updated_at = CURRENT_TIMESTAMP(6) \
             WHERE follow_up_id = ? AND account_id = ? AND status = 'scheduled' \
               AND source_version = ?",
            [
                follow_up_id.into(),
                account_id.into(),
                expected_source_version.into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if updated.rows_affected() != 1 {
        return Err(FollowUpControlStoreError::InvalidData(
            "follow-up compare-and-set failed".into(),
        ));
    }
    suppress_pending_outbox(
        db,
        account_id,
        Some(follow_up_id),
        "follow_up",
        follow_up_id,
    )
    .await?;
    Ok(AppliedControl {
        follow_up_id: follow_up_id.to_owned(),
        control_kind: "complete",
        previous_status: "scheduled",
        current_status: "completed",
        previous_source_version: item.source_version,
        current_source_version: item.source_version + 1,
        previous_due_at_unix_secs: None,
        current_due_at_unix_secs: None,
        reason,
        result_ref: format!(
            "跟进事项 {} 已完成（版本 {} -> {}）",
            follow_up_id,
            item.source_version,
            item.source_version + 1
        ),
    })
}

/// 批量完成：按 follow_up_id 字典序锁定（确定性锁顺序，避免重叠批次死锁），
/// 先验证全部目标与关联 Outbox 再执行任何业务 UPDATE；任一失败整个事务回滚，
/// 不允许部分成功（all-or-nothing）。
pub(super) async fn apply_batch_complete<C: ConnectionTrait>(
    db: &C,
    request: &FollowUpControlEffectRequest,
    account_id: u64,
) -> Result<AppliedControlBatch, FollowUpControlStoreError> {
    let (targets, reason) = match &request.action {
        SecretaryAction::CompleteFollowUps { targets, reason } => (targets, reason.clone()),
        _ => {
            return Err(FollowUpControlStoreError::InvalidData(
                "action is not a batch follow-up complete".into(),
            ));
        }
    };
    // 确定性锁顺序：不能按 LLM 给出的原始顺序直接锁库。
    let mut ordered: Vec<&FollowUpControlTarget> = targets.iter().collect();
    ordered.sort_by(|a, b| a.follow_up_id.as_str().cmp(b.follow_up_id.as_str()));

    // 阶段 1：锁定全部目标并校验状态/版本；任一不存在/不匹配立即失败。
    let mut locked = Vec::with_capacity(ordered.len());
    for target in &ordered {
        let item = FollowUpItemRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT status, due_at_unix_secs, source_version FROM secretary_follow_up_items \
             WHERE follow_up_id = ? AND account_id = ? FOR UPDATE",
            [target.follow_up_id.as_str().into(), account_id.into()],
        ))
        .one(db)
        .await
        .map_err(database_error)?
        .ok_or_else(|| {
            FollowUpControlStoreError::InvalidData("follow_up not found in account".into())
        })?;
        if item.status != "scheduled" || item.source_version != target.expected_source_version {
            return Err(FollowUpControlStoreError::InvalidData(
                "follow_up status or source_version changed since approval".into(),
            ));
        }
        locked.push((target, item));
    }
    // 阶段 2：锁定全部关联 Outbox（legacy + policy-owned 回溯）并拒绝 claimed。
    for target in &ordered {
        lock_and_check_outbox(
            db,
            account_id,
            Some(target.follow_up_id.as_str()),
            "follow_up",
            target.follow_up_id.as_str(),
            "complete",
        )
        .await?;
    }
    // 阶段 3：全部校验通过后才执行 CAS 更新（status -> completed，version 精确 +1，
    // due 不变）。
    for (target, _) in &locked {
        let updated = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_follow_up_items \
                 SET status = 'completed', source_version = source_version + 1, \
                     updated_at = CURRENT_TIMESTAMP(6) \
                 WHERE follow_up_id = ? AND account_id = ? AND status = 'scheduled' \
                   AND source_version = ?",
                [
                    target.follow_up_id.as_str().into(),
                    account_id.into(),
                    target.expected_source_version.into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        if updated.rows_affected() != 1 {
            return Err(FollowUpControlStoreError::InvalidData(
                "follow-up compare-and-set failed".into(),
            ));
        }
    }
    // 阶段 4：压制全部目标的 pending/failed Outbox 并清除租约；delivered 保留。
    for target in &ordered {
        suppress_pending_outbox(
            db,
            account_id,
            Some(target.follow_up_id.as_str()),
            "follow_up",
            target.follow_up_id.as_str(),
        )
        .await?;
    }
    // 阶段 5：组装每目标审计与有界结果文案（只含数量与 FollowUp ID）。
    let mut controls = Vec::with_capacity(locked.len());
    for (target, item) in &locked {
        controls.push(AppliedControl {
            follow_up_id: target.follow_up_id.as_str().to_owned(),
            control_kind: "complete",
            previous_status: "scheduled",
            current_status: "completed",
            previous_source_version: item.source_version,
            current_source_version: item.source_version + 1,
            previous_due_at_unix_secs: None,
            current_due_at_unix_secs: None,
            reason: reason.clone(),
            result_ref: format!(
                "跟进事项 {} 已完成（版本 {} -> {}）",
                target.follow_up_id.as_str(),
                item.source_version,
                item.source_version + 1
            ),
        });
    }
    // 20 个目标 × 37 字符仍在响应有界约束内；不包含账号 ID/OpenID/Token/聊天正文。
    let ids = controls
        .iter()
        .map(|control| control.follow_up_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let result_ref = format!("已批量完成 {} 条跟进事项：{ids}", controls.len());
    Ok(AppliedControlBatch {
        controls,
        result_ref,
    })
}

#[derive(FromQueryResult)]
struct FollowUpItemRow {
    status: String,
    due_at_unix_secs: i64,
    source_version: u64,
}

#[derive(FromQueryResult)]
struct TimeWindowRow {
    is_future: i64,
    within_365_days: i64,
}
