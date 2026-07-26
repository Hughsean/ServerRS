//! Row DTO、账号解析与领取行映射。
//!
//! `BIGINT UNSIGNED` 列必须用 `u64` 解码；MySQL JSON 列必须 `CAST(... AS CHAR)`
//! 才能用 `String` 解码（教训：sqlx/sea-orm 对 JSON 列直接反序列化会假阴性）。
//! 账号条件在数据库查询边界，不只是上层过滤。

use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
use tracing::debug;

use super::super::mysql_inbound::store_error;
use crate::{
    ActionLeaseToken, ActionRunId, ActionStoreError, ClaimedActionRun, MessageSource,
    RecentEventRef, SecretaryActionReceipt, SourceAccountRef, SourceEventId,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};

#[derive(Debug, FromQueryResult)]
pub(super) struct AccountIdRow {
    pub(super) id: u64,
}

#[derive(Debug, FromQueryResult)]
pub(super) struct ExistingRunRow {
    pub(super) run_id: String,
}

#[allow(dead_code)]
#[derive(Debug, FromQueryResult)]
pub(super) struct ClaimedRunRow {
    pub(super) run_id: String,
    pub(super) account_id: u64,
    pub(super) command_source_event_id: String,
    pub(super) command_text: String,
    pub(super) conversation_id: String,
    pub(super) occurred_at_unix_secs: i64,
    pub(super) timezone_offset_secs: i64,
    pub(super) recent_events_json: Option<String>,
    pub(super) lease_token: String,
    pub(super) source_channel: String,
    pub(super) platform_account_id: String,
}

#[derive(Debug, FromQueryResult)]
pub(super) struct CheckpointRow {
    pub(super) last_checkpoint_json: Option<String>,
}

#[derive(Debug, FromQueryResult)]
pub(super) struct EffectReceiptRow {
    pub(super) proposal_json: String,
    pub(super) result_ref: String,
}

pub(super) async fn resolve_account_id(
    db: &DatabaseConnection,
    account: &SourceAccountRef,
) -> Result<u64, ActionStoreError> {
    AccountIdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ? AND status = 'active'",
        [
            account.channel.as_str().into(),
            account.account_id.clone().into(),
        ],
    ))
    .one(db)
    .await
    .map_err(store_error)?
    .map(|r| r.id)
    .ok_or_else(|| {
        ActionStoreError::InvalidData(format!(
            "account not found: {}/{})",
            account.channel.as_str(),
            account.account_id
        ))
    })
}

pub(super) fn map_claimed_row(
    row: ClaimedRunRow,
    lease_token: ActionLeaseToken,
) -> Result<Option<ClaimedActionRun>, ActionStoreError> {
    let recent_events: Vec<RecentEventRef> = row
        .recent_events_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let channel = match row.source_channel.as_str() {
        "napcat" => MessageSource::NapCat,
        "qq_open_platform" => MessageSource::QqOpenPlatform,
        other => {
            return Err(ActionStoreError::InvalidData(format!(
                "unknown source_channel: {other}"
            )));
        }
    };
    let account = SourceAccountRef::new(channel, &row.platform_account_id)
        .map_err(|e| ActionStoreError::InvalidData(e.to_string()))?;
    Ok(Some(ClaimedActionRun {
        run_id: ActionRunId::new(&row.run_id)?,
        lease_token,
        account,
        command_source_event_id: SourceEventId::new(&row.command_source_event_id)?,
        command_text: row.command_text,
        conversation_id: row.conversation_id,
        occurred_at_unix_secs: row.occurred_at_unix_secs,
        timezone_offset_secs: row.timezone_offset_secs,
        recent_events,
    }))
}

pub(super) async fn load_effect_receipt_from<C>(
    connection: &C,
    run_id: &ActionRunId,
    effect_id: &str,
) -> Result<Option<SecretaryActionReceipt>, ActionStoreError>
where
    C: ConnectionTrait,
{
    let row = EffectReceiptRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT CAST(proposal_json AS CHAR) AS proposal_json, result_ref FROM secretary_action_effect_receipts WHERE run_id = ? AND effect_id = ?",
        vec![run_id.as_str().into(), effect_id.into()],
    ))
    .one(connection)
    .await
    .map_err(store_error)?;
    row.map(|row| {
        let proposal: crate::SecretaryActionProposal = serde_json::from_str(&row.proposal_json)
            .map_err(|error| ActionStoreError::InvalidData(error.to_string()))?;
        debug!(
            run_id = run_id.as_str(),
            effect_id, "loaded existing effect receipt"
        );
        Ok(SecretaryActionReceipt {
            proposal_id: proposal.proposal_id,
            result_ref: row.result_ref,
        })
    })
    .transpose()
}
