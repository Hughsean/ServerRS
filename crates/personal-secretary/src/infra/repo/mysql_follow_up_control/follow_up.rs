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
/// 同时关闭承诺生命周期缺口（MEM-004 B3）：若 FollowUp 来源是承诺记忆
/// （reason_code = 'commitment_due'），在同事务内把旧 Pending Commitment Fact
/// supersede 并落一条新的 Confirmed Fulfilled Commitment Fact，
/// `completion_source_event_id` 指向本次授权 OwnerCommand。
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
    let item = CompleteFollowUpRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT status, due_at_unix_secs, source_version, reason_code, \
                source_memory_fact_id \
         FROM secretary_follow_up_items \
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
    // B3: 若 FollowUp 来源是承诺记忆，闭合一致性缺口。
    // 只有承诺类 FollowUp 才需要更新底层 Commitment Fact；
    // 项目阻塞类 FollowUp 不涉及承诺状态。
    // fail-closed：承诺类必须有 source_memory_fact_id，缺失不得继续完成。
    let commitment_closed = if item.reason_code == "commitment_due" {
        let fact_id = item.source_memory_fact_id.as_deref().ok_or_else(|| {
            FollowUpControlStoreError::InvalidData(
                "commitment_due follow_up is missing source_memory_fact_id".into(),
            )
        })?;
        close_commitment_on_complete(
            db,
            fact_id,
            account_id,
            request.command_source_event_id.as_str(),
            &request.effect_id,
            follow_up_id,
        )
        .await?
    } else {
        false
    };
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
    let result_ref = if commitment_closed {
        format!(
            "跟进事项 {} 已完成，关联承诺已标记为已履行（版本 {} -> {}）",
            follow_up_id,
            item.source_version,
            item.source_version + 1
        )
    } else {
        format!(
            "跟进事项 {} 已完成（版本 {} -> {}）",
            follow_up_id,
            item.source_version,
            item.source_version + 1
        )
    };
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
        result_ref,
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
    // 使用 CompleteFollowUpRow 同时获取 reason_code 与 source_memory_fact_id，
    // 用于后续承诺闭环（MEM-004 B3）。
    struct BatchCompleteItem<'a> {
        target: &'a FollowUpControlTarget,
        source_version: u64,
        reason_code: String,
        source_memory_fact_id: Option<String>,
    }
    let mut locked = Vec::with_capacity(ordered.len());
    for target in &ordered {
        let item = CompleteFollowUpRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT status, due_at_unix_secs, source_version, reason_code, \
                    source_memory_fact_id \
             FROM secretary_follow_up_items \
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
        locked.push(BatchCompleteItem {
            target,
            source_version: item.source_version,
            reason_code: item.reason_code,
            source_memory_fact_id: item.source_memory_fact_id,
        });
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
    // 阶段 2.5：关闭承诺生命周期缺口（MEM-004 B3）。
    // 必须在 Outbox 锁定后、FollowUp 状态 UPDATE 前执行，保证 all-or-nothing。
    // fail-closed：承诺类必须有 source_memory_fact_id。
    for item in &locked {
        if item.reason_code == "commitment_due" {
            let fact_id = item.source_memory_fact_id.as_deref().ok_or_else(|| {
                FollowUpControlStoreError::InvalidData(
                    "commitment_due follow_up is missing source_memory_fact_id".into(),
                )
            })?;
            close_commitment_on_complete(
                db,
                fact_id,
                account_id,
                request.command_source_event_id.as_str(),
                &request.effect_id,
                item.target.follow_up_id.as_str(),
            )
            .await?;
        }
    }
    // 阶段 3：全部校验通过后才执行 CAS 更新（status -> completed，version 精确 +1，
    // due 不变）。
    for item in &locked {
        let updated = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_follow_up_items \
                 SET status = 'completed', source_version = source_version + 1, \
                     updated_at = CURRENT_TIMESTAMP(6) \
                 WHERE follow_up_id = ? AND account_id = ? AND status = 'scheduled' \
                   AND source_version = ?",
                [
                    item.target.follow_up_id.as_str().into(),
                    account_id.into(),
                    item.target.expected_source_version.into(),
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
    for item in &locked {
        controls.push(AppliedControl {
            follow_up_id: item.target.follow_up_id.as_str().to_owned(),
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
                item.target.follow_up_id.as_str(),
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

/// `apply_complete` 专用行模型：比 FollowUpItemRow 多取 `reason_code` 与
/// `source_memory_fact_id`，用于判定是否需关闭承诺一致性缺口。
#[derive(FromQueryResult)]
struct CompleteFollowUpRow {
    status: String,
    #[allow(dead_code)]
    due_at_unix_secs: i64,
    source_version: u64,
    reason_code: String,
    source_memory_fact_id: Option<String>,
}

#[derive(FromQueryResult)]
struct TimeWindowRow {
    is_future: i64,
    within_365_days: i64,
}

/// `close_commitment_on_complete` 专用：锁定时需拿到 fact_status 与 fact_json。
#[derive(FromQueryResult)]
struct CommitmentCloseRow {
    fact_json: String,
    fact_status: String,
}

/// 完成 FollowUp 时闭合承诺生命周期缺口（MEM-004 B3）。
///
/// 在同一事务内：
/// 1. 锁定来源 Commitment Fact（Pending 状态）；
/// 2. 校验仍属本账号、仍为 Pending 且未被并发 supersede；
/// 3. 落新 Confirmed Fulfilled Commitment Fact，`completion_source_event_id` 指向
///    本次授权 OwnerCommand，`supersedes_fact_id` 指回旧 Fact；
/// 4. 旧 Pending Fact → superseded。
///
/// 任一步失败全部回滚（调用方在同一事务内）。
async fn close_commitment_on_complete<C: ConnectionTrait>(
    db: &C,
    source_fact_id: &str,
    account_id: u64,
    command_source_event_id: &str,
    effect_id: &str,
    follow_up_id: &str,
) -> Result<bool, FollowUpControlStoreError> {
    // 1. 锁定来源 Commitment Fact
    let fact = CommitmentCloseRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT CAST(fact_json AS CHAR) AS fact_json, fact_status \
             FROM secretary_memory_facts \
             WHERE fact_id = ? AND account_id = ? AND fact_kind = 'commitment' \
               AND fact_status = 'confirmed' \
             FOR UPDATE",
        [source_fact_id.into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or_else(|| {
        FollowUpControlStoreError::InvalidData(
            "source commitment fact not found or is not active".into(),
        )
    })?;
    if fact.fact_status != "confirmed" {
        return Err(FollowUpControlStoreError::InvalidData(
            "source commitment fact is no longer confirmed".into(),
        ));
    }
    let mem_fact: crate::MemoryFact = serde_json::from_str(&fact.fact_json).map_err(|error| {
        FollowUpControlStoreError::InvalidData(format!(
            "stored commitment fact is invalid: {error}"
        ))
    })?;
    let commitment = match &mem_fact.payload {
        crate::MemoryPayload::Commitment(c) => c,
        _ => {
            return Err(FollowUpControlStoreError::InvalidData(
                "source fact is not a commitment".into(),
            ));
        }
    };
    if commitment.status != crate::CommitmentStatus::Pending {
        // 已经是 Fulfilled/Cancelled：承诺生命周期已由并发 Effect 关闭，
        // 本次完成仍然写 FollowUp completed，但不重复创建 Fulfilled Fact。
        return Ok(false);
    }

    // 2. 构造 Fulfilled Commitment Fact（重建完整 MemoryFact）
    let new_fact_id = crate::MemoryFactId::new(super::authorization::stable_id(
        "commitment-fulfilled",
        &format!("{}\0{}", effect_id, follow_up_id),
    ))
    .map_err(|e| FollowUpControlStoreError::InvalidData(e.to_string()))?;
    let old_fact_id = crate::MemoryFactId::new(source_fact_id)
        .map_err(|e| FollowUpControlStoreError::InvalidData(e.to_string()))?;
    let completion_evt = crate::SourceEventId::new(command_source_event_id)
        .map_err(|e| FollowUpControlStoreError::InvalidData(e.to_string()))?;
    // 更新 payload 中的承诺状态
    let mut new_payload = commitment.clone();
    new_payload.status = crate::CommitmentStatus::Fulfilled;
    new_payload.completion_source_event_id = Some(completion_evt.clone());
    // 追加 completion_source_event_id 到来源列表
    let mut new_source_ids = mem_fact.source_event_ids.clone();
    if !new_source_ids.contains(&completion_evt) {
        new_source_ids.push(completion_evt);
    }
    let new_fact = crate::MemoryFact {
        fact_id: new_fact_id.clone(),
        account: mem_fact.account.clone(),
        subject_key: mem_fact.subject_key.clone(),
        payload: crate::MemoryPayload::Commitment(new_payload),
        status: crate::MemoryFactStatus::Confirmed,
        confidence_bps: mem_fact.confidence_bps,
        source_event_ids: new_source_ids,
        valid_until_unix_secs: mem_fact.valid_until_unix_secs,
        supersedes_fact_id: Some(old_fact_id.clone()),
    };
    // 写入前通过领域校验（subject_key、status、confidence_bps 等硬约束）。
    crate::validate_memory_fact(&new_fact).map_err(|e| {
        FollowUpControlStoreError::InvalidData(format!(
            "fulfilled commitment fact validation failed: {e}"
        ))
    })?;
    let new_fact_json = serde_json::to_string(&new_fact).map_err(|error| {
        FollowUpControlStoreError::InvalidData(format!(
            "cannot serialize fulfilled commitment: {error}"
        ))
    })?;
    // 3. 标记旧 Pending Fact → superseded
    let superseded = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_memory_facts \
             SET fact_status = 'superseded' \
             WHERE fact_id = ? AND account_id = ? AND fact_status = 'confirmed'",
            [source_fact_id.into(), account_id.into()],
        ))
        .await
        .map_err(database_error)?;
    if superseded.rows_affected() != 1 {
        return Err(FollowUpControlStoreError::InvalidData(
            "commitment fact supersede CAS failed".into(),
        ));
    }
    // 4. 落新 Confirmed Fulfilled Fact
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_memory_facts \
             (fact_id, account_id, fact_kind, subject_key, fact_json, fact_status, \
              confidence_bps, valid_until_unix_secs, supersedes_fact_id) \
             SELECT ?, account_id, fact_kind, subject_key, ?, 'confirmed', \
                    confidence_bps, valid_until_unix_secs, ? \
             FROM secretary_memory_facts \
             WHERE fact_id = ? AND account_id = ?",
        [
            new_fact_id.as_str().into(),
            new_fact_json.into(),
            old_fact_id.as_str().into(),
            source_fact_id.into(),
            account_id.into(),
        ],
    ))
    .await
    .map_err(database_error)?;
    // 5. 复制来源引用到新 Fact
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_memory_fact_sources (fact_id, source_event_id) \
             SELECT ?, source_event_id \
             FROM secretary_memory_fact_sources WHERE fact_id = ?",
        [new_fact_id.as_str().into(), source_fact_id.into()],
    ))
    .await
    .map_err(database_error)?;
    // 6. 追加 completion_source_event_id 到来源列表
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT IGNORE INTO secretary_memory_fact_sources (fact_id, source_event_id) \
         VALUES (?, ?)",
        [new_fact_id.as_str().into(), command_source_event_id.into()],
    ))
    .await
    .map_err(database_error)?;
    Ok(true)
}
