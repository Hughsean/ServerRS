//! 关联通知（Outbox）的锁定、状态拒绝与压制。
//!
//! FollowUp 同时覆盖 legacy `outbox.follow_up_id` 直连与 policy-owned
//! Candidate 回溯；ResponseExpectation 只经 Candidate 回溯
//! （`source_kind = 'response_expectation'`），legacy 参数传 None 即不匹配任何行。
//! 行锁消除“检查后、压制前”的投递竞态；claimed/unknown_commit 必须整批拒绝，
//! delivered 历史保留，不删除也不改写。

use sea_orm::{ConnectionTrait, DatabaseBackend, FromQueryResult, Statement};

use super::authorization::{ControlAuthError, database_error};

#[derive(FromQueryResult)]
pub(super) struct OutboxStatusRow {
    delivery_status: String,
}

/// 锁定该来源关联的全部 Outbox 行，任一为 claimed/unknown_commit 即拒绝；
/// `verb` 只用于错误文案。
pub(crate) async fn lock_and_check_outbox<C: ConnectionTrait>(
    db: &C,
    account_id: u64,
    legacy_follow_up_id: Option<&str>,
    candidate_kind: &str,
    candidate_source_id: &str,
    verb: &str,
) -> Result<(), ControlAuthError> {
    let outbox_rows = OutboxStatusRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT outbox.delivery_status \
         FROM secretary_notification_outbox outbox \
         LEFT JOIN secretary_notification_candidates candidate \
           ON candidate.notification_candidate_id = outbox.notification_candidate_id \
         WHERE outbox.account_id = ? AND (outbox.follow_up_id = ? OR \
               (candidate.source_kind = ? AND candidate.source_id = ?)) \
         ORDER BY outbox.notification_id \
         FOR UPDATE",
        [
            account_id.into(),
            legacy_follow_up_id.map(str::to_owned).into(),
            candidate_kind.into(),
            candidate_source_id.into(),
        ],
    ))
    .all(db)
    .await
    .map_err(database_error)?;
    if outbox_rows
        .iter()
        .any(|row| matches!(row.delivery_status.as_str(), "claimed" | "unknown_commit"))
    {
        return Err(ControlAuthError::InvalidData(format!(
            "{candidate_kind} has claimed or unknown_commit outbox rows; cannot safely {verb}"
        )));
    }
    Ok(())
}

/// 把相关 pending/failed 的 Owner Outbox 转为 suppressed 并清除租约；
/// delivered 历史保留，不删除也不改写。
pub(crate) async fn suppress_pending_outbox<C: ConnectionTrait>(
    db: &C,
    account_id: u64,
    legacy_follow_up_id: Option<&str>,
    candidate_kind: &str,
    candidate_source_id: &str,
) -> Result<(), ControlAuthError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_notification_outbox outbox \
         LEFT JOIN secretary_notification_candidates candidate \
           ON candidate.notification_candidate_id = outbox.notification_candidate_id \
         SET outbox.delivery_status = 'suppressed', outbox.lease_token = NULL, \
             outbox.lease_expires_at = NULL \
         WHERE outbox.account_id = ? AND (outbox.follow_up_id = ? OR \
               (candidate.source_kind = ? AND candidate.source_id = ?)) \
           AND outbox.delivery_status IN ('pending', 'failed')",
        [
            account_id.into(),
            legacy_follow_up_id.map(str::to_owned).into(),
            candidate_kind.into(),
            candidate_source_id.into(),
        ],
    ))
    .await
    .map_err(database_error)?;
    Ok(())
}
