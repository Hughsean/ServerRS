use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};

use crate::{
    FollowUpControlEffectRequest, FollowUpControlStoreError, FollowUpControlStoreT,
    FollowUpControlTarget, SecretaryAction, SecretaryActionProposal, SecretaryActionReceipt,
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
        if let Some(receipt) = load_receipt(&transaction, request).await? {
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }
        let account_id = lock_account(&transaction, request).await?;
        verify_action_lease(&transaction, request, account_id).await?;
        verify_owner_command(&transaction, request, account_id).await?;
        // 竞争窗口内可能已有并发 Effect 写入回执；提交前再校验一次碰撞。
        if let Some(receipt) = load_receipt(&transaction, request).await? {
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }

        // 单条 Dismiss/Snooze 包装为长度 1 的批次，业务行为与结果文案保持不变；
        // DismissFollowUps 走真正的批量 all-or-nothing 路径。
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
            SecretaryAction::DismissFollowUps { .. } => {
                apply_batch_dismiss(&transaction, request, account_id).await?
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
            SecretaryAction::DismissFollowUp { .. } | SecretaryAction::SnoozeFollowUp { .. }
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
            insert_control_audit(&transaction, request, account_id, control, &control_id).await?;
        }
        let inserted = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT IGNORE INTO secretary_action_effect_receipts \
                 (effect_id, run_id, proposal_json, result_ref) VALUES (?, ?, ?, ?)",
                [
                    request.effect_id.clone().into(),
                    request.run_id.as_str().into(),
                    request.proposal_json.clone().into(),
                    applied.result_ref.clone().into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        if inserted.rows_affected() != 1 {
            // 回执被并发抢先写入：加载并校验归属，而不是盲目宣告成功。
            let receipt = load_receipt(&transaction, request)
                .await?
                .ok_or(FollowUpControlStoreError::Database)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(SecretaryActionReceipt {
            proposal_id: request.proposal_id.clone(),
            result_ref: applied.result_ref,
            tool_kind: Some(request.action.kind()),
        })
    }
}

/// 两种 FollowUp 控制的统一落库结果；dismiss 与 snooze 共用审计与回执写入。
struct AppliedControl {
    follow_up_id: String,
    control_kind: &'static str,
    previous_status: &'static str,
    current_status: &'static str,
    previous_source_version: u64,
    current_source_version: u64,
    previous_due_at_unix_secs: Option<i64>,
    current_due_at_unix_secs: Option<i64>,
    reason: String,
    result_ref: String,
}

/// 一次 Effect 的完整落库结果：单条 Dismiss/Snooze 包装为长度 1 的批次，
/// 批量 Dismiss 为多个目标；整个批次只写一条通用 Effect Receipt。
struct AppliedControlBatch {
    controls: Vec<AppliedControl>,
    result_ref: String,
}

/// 锁定 FollowUp 并执行忽略：状态/版本 CAS、Outbox 状态拒绝与压制。
async fn apply_dismiss<C: ConnectionTrait>(
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
    lock_and_check_outbox(db, account_id, follow_up_id, "dismiss").await?;
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
    suppress_pending_outbox(db, account_id, follow_up_id).await?;
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

/// 批量忽略：按 follow_up_id 字典序锁定（确定性锁顺序，避免重叠批次死锁），
/// 先验证全部目标与关联 Outbox 再执行任何业务 UPDATE；任一失败整个事务回滚，
/// 不允许部分成功（all-or-nothing）。
async fn apply_batch_dismiss<C: ConnectionTrait>(
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
        lock_and_check_outbox(db, account_id, target.follow_up_id.as_str(), "dismiss").await?;
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
        suppress_pending_outbox(db, account_id, target.follow_up_id.as_str()).await?;
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

/// 锁定 FollowUp 并执行推迟：时间窗口、状态/版本 CAS、Outbox 状态拒绝与压制。
async fn apply_snooze<C: ConnectionTrait>(
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
    lock_and_check_outbox(db, account_id, follow_up_id, "snooze").await?;
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
    suppress_pending_outbox(db, account_id, follow_up_id).await?;
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

/// 锁定该 FollowUp 关联的全部 Outbox 行（legacy follow_up_id + policy-owned
/// candidate 回溯），任一为 claimed/unknown_commit 即拒绝；`verb` 只用于错误文案。
/// 行锁消除“检查后、压制前”的投递竞态。
async fn lock_and_check_outbox<C: ConnectionTrait>(
    db: &C,
    account_id: u64,
    follow_up_id: &str,
    verb: &str,
) -> Result<(), FollowUpControlStoreError> {
    let outbox_rows = OutboxStatusRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT outbox.delivery_status \
         FROM secretary_notification_outbox outbox \
         LEFT JOIN secretary_notification_candidates candidate \
           ON candidate.notification_candidate_id = outbox.notification_candidate_id \
         WHERE outbox.account_id = ? AND (outbox.follow_up_id = ? OR \
               (candidate.source_kind = 'follow_up' AND candidate.source_id = ?)) \
         ORDER BY outbox.notification_id \
         FOR UPDATE",
        [account_id.into(), follow_up_id.into(), follow_up_id.into()],
    ))
    .all(db)
    .await
    .map_err(database_error)?;
    if outbox_rows
        .iter()
        .any(|row| matches!(row.delivery_status.as_str(), "claimed" | "unknown_commit"))
    {
        return Err(FollowUpControlStoreError::InvalidData(format!(
            "follow-up has claimed or unknown_commit outbox rows; cannot safely {verb}"
        )));
    }
    Ok(())
}

/// 把相关 pending/failed 的 Owner Outbox 转为 suppressed 并清除租约；
/// delivered 历史保留，不删除也不改写。
async fn suppress_pending_outbox<C: ConnectionTrait>(
    db: &C,
    account_id: u64,
    follow_up_id: &str,
) -> Result<(), FollowUpControlStoreError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_notification_outbox outbox \
         LEFT JOIN secretary_notification_candidates candidate \
           ON candidate.notification_candidate_id = outbox.notification_candidate_id \
         SET outbox.delivery_status = 'suppressed', outbox.lease_token = NULL, \
             outbox.lease_expires_at = NULL \
         WHERE outbox.account_id = ? AND (outbox.follow_up_id = ? OR \
               (candidate.source_kind = 'follow_up' AND candidate.source_id = ?)) \
           AND outbox.delivery_status IN ('pending', 'failed')",
        [account_id.into(), follow_up_id.into(), follow_up_id.into()],
    ))
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn insert_control_audit<C: ConnectionTrait>(
    db: &C,
    request: &FollowUpControlEffectRequest,
    account_id: u64,
    applied: &AppliedControl,
    control_id: &str,
) -> Result<(), FollowUpControlStoreError> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_follow_up_owner_controls \
             (control_id, effect_id, run_id, proposal_id, account_id, follow_up_id, \
              previous_status, current_status, previous_source_version, current_source_version, \
              command_source_event_id, reason, control_kind, previous_due_at_unix_secs, \
              current_due_at_unix_secs) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            [
                control_id.into(),
                request.effect_id.clone().into(),
                request.run_id.as_str().into(),
                request.proposal_id.clone().into(),
                account_id.into(),
                applied.follow_up_id.clone().into(),
                applied.previous_status.into(),
                applied.current_status.into(),
                applied.previous_source_version.into(),
                applied.current_source_version.into(),
                request.command_source_event_id.as_str().into(),
                applied.reason.clone().into(),
                applied.control_kind.into(),
                applied.previous_due_at_unix_secs.into(),
                applied.current_due_at_unix_secs.into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err(FollowUpControlStoreError::Database);
    }
    Ok(())
}

async fn lock_account<C: ConnectionTrait>(
    db: &C,
    request: &FollowUpControlEffectRequest,
) -> Result<u64, FollowUpControlStoreError> {
    IdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id FROM secretary_accounts WHERE source_channel = ? \
         AND platform_account_id = ? AND status = 'active' FOR UPDATE",
        [
            request.account.channel.as_str().into(),
            request.account.account_id.clone().into(),
        ],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .map(|row| row.id)
    .ok_or(FollowUpControlStoreError::Unauthorized)
}

async fn verify_action_lease<C: ConnectionTrait>(
    db: &C,
    request: &FollowUpControlEffectRequest,
    account_id: u64,
) -> Result<(), FollowUpControlStoreError> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_action_runs SET updated_at = UTC_TIMESTAMP(6) \
             WHERE run_id = ? AND lease_token = ? AND status = 'running' AND account_id = ? \
               AND command_source_event_id = ? AND lease_expires_at >= UTC_TIMESTAMP(6)",
            [
                request.run_id.as_str().into(),
                request.lease_token.as_str().into(),
                account_id.into(),
                request.command_source_event_id.as_str().into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err(FollowUpControlStoreError::LeaseLost);
    }
    Ok(())
}

async fn verify_owner_command<C: ConnectionTrait>(
    db: &C,
    request: &FollowUpControlEffectRequest,
    managed_account_id: u64,
) -> Result<(), FollowUpControlStoreError> {
    let command = CommandRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT account_id, actor_platform_id, message_role FROM secretary_source_events \
         WHERE source_event_id = ? FOR UPDATE",
        [request.command_source_event_id.as_str().into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or(FollowUpControlStoreError::Unauthorized)?;
    if command.message_role != "owner_command" {
        return Err(FollowUpControlStoreError::Unauthorized);
    }
    let bindings = BindingRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT command_account_id, owner_actor_id FROM secretary_owner_bindings \
         WHERE managed_account_id = ? AND status = 'active' LIMIT 2 FOR UPDATE",
        [managed_account_id.into()],
    ))
    .all(db)
    .await
    .map_err(database_error)?;
    match bindings.as_slice() {
        [binding]
            if binding.command_account_id == command.account_id
                && binding.owner_actor_id == command.actor_platform_id =>
        {
            Ok(())
        }
        _ => Err(FollowUpControlStoreError::Unauthorized),
    }
}

async fn load_receipt<C: ConnectionTrait>(
    db: &C,
    request: &FollowUpControlEffectRequest,
) -> Result<Option<SecretaryActionReceipt>, FollowUpControlStoreError> {
    let row = ReceiptRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT run_id, CAST(proposal_json AS CHAR) AS proposal_json, result_ref \
         FROM secretary_action_effect_receipts WHERE effect_id = ?",
        [request.effect_id.clone().into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?;
    row.map(|row| {
        let proposal: SecretaryActionProposal = serde_json::from_str(&row.proposal_json)
            .map_err(|_| FollowUpControlStoreError::Database)?;
        if row.run_id != request.run_id.as_str()
            || proposal.proposal_id != request.proposal_id
            || proposal.action != request.action
        {
            return Err(FollowUpControlStoreError::InvalidData(
                "effect receipt belongs to a different action".into(),
            ));
        }
        Ok(SecretaryActionReceipt {
            proposal_id: proposal.proposal_id,
            result_ref: row.result_ref,
            tool_kind: Some(request.action.kind()),
        })
    })
    .transpose()
}

fn stable_id(namespace: &str, effect_id: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("{namespace}:{effect_id}").as_bytes(),
    )
    .to_string()
}

fn database_error(_: sea_orm::DbErr) -> FollowUpControlStoreError {
    FollowUpControlStoreError::Database
}

#[derive(FromQueryResult)]
struct IdRow {
    id: u64,
}

#[derive(FromQueryResult)]
struct CommandRow {
    account_id: u64,
    actor_platform_id: String,
    message_role: String,
}

#[derive(FromQueryResult)]
struct BindingRow {
    command_account_id: u64,
    owner_actor_id: String,
}

#[derive(FromQueryResult)]
struct ReceiptRow {
    run_id: String,
    proposal_json: String,
    result_ref: String,
}

#[derive(FromQueryResult)]
struct FollowUpItemRow {
    status: String,
    due_at_unix_secs: i64,
    source_version: u64,
}

#[derive(FromQueryResult)]
struct OutboxStatusRow {
    delivery_status: String,
}

#[derive(FromQueryResult)]
struct TimeWindowRow {
    is_future: i64,
    within_365_days: i64,
}
