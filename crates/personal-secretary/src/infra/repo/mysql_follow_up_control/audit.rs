//! FollowUp 控制审计写入：每目标一行不可变审计，共享同一 effect_id。
//!
//! 审计行由 `(effect_id, follow_up_id)` 复合唯一键保证重放不重复；行内记录
//! 前后状态、前后版本与前后 due（dismiss/complete 为 NULL），版本必须精确 +1。

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use crate::{FollowUpControlEffectRequest, FollowUpControlStoreError};

use super::authorization::database_error;
use super::follow_up::AppliedControl;

pub(super) async fn insert_control_audit<C: ConnectionTrait>(
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
