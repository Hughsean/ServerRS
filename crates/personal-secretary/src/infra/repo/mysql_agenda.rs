use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};

use crate::{
    AgendaApplyRequest, AgendaError, AgendaItem, AgendaItemId, AgendaItemKind, AgendaItemStatus,
    AgendaMutation, AgendaMutationReceipt, AgendaStoreT, NotificationCandidateProductionReport,
    SourceAccountRef, SourceEventId,
};

use super::mysql_inbound::store_error;
use super::mysql_notification_candidate_producer::{
    LockedNotificationSource, produce_from_locked_source,
};

pub(crate) struct MySqlAgendaStore {
    db: DatabaseConnection,
}

impl MySqlAgendaStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AgendaStoreT for MySqlAgendaStore {
    async fn apply(
        &self,
        request: &AgendaApplyRequest,
        _now_unix_secs: i64,
    ) -> Result<AgendaMutationReceipt, AgendaError> {
        if let Some(row) = ReceiptRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT result_ref FROM secretary_action_effect_receipts WHERE run_id = ? AND effect_id = ?",
            [request.run_id.clone().into(), request.effect_id.clone().into()],
        ))
        .one(&self.db)
        .await
        .map_err(map_db)?
        {
            let item_id = AgendaItemId::new(
                row.result_ref
                    .split(':')
                    .nth(1)
                    .ok_or_else(|| AgendaError::Store("invalid agenda result_ref".into()))?,
            )?;
            let account_id = resolve_account_id(&self.db, &request.account).await?;
            let item = map_row(
                load_item(&self.db, account_id, &item_id).await?,
                request.account.clone(),
            )?;
            return Ok(AgendaMutationReceipt {
                item,
                result_ref: row.result_ref,
            });
        }
        let transaction = self.db.begin().await.map_err(map_db)?;
        let account_id = resolve_account_id(&transaction, &request.account).await?;
        let lease = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_action_runs SET updated_at = UTC_TIMESTAMP(6) WHERE run_id = ? AND lease_token = ? AND status = 'running' AND account_id = ?",
                [request.run_id.clone().into(), request.lease_token.clone().into(), account_id.into()],
            ))
            .await
            .map_err(map_db)?;
        if lease.rows_affected() != 1 {
            return Err(AgendaError::Store("action lease lost".into()));
        }

        let (item, from_version) = match &request.mutation {
            AgendaMutation::Create {
                kind,
                title,
                scheduled_at_unix_secs,
                timezone,
            } => {
                let item_id = AgendaItemId::generate();
                transaction
                    .execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::MySql,
                        r#"INSERT IGNORE INTO secretary_agenda_items
                           (item_id, account_id, item_kind, title, scheduled_at_unix_secs,
                            timezone_name, item_status, version, created_command_event_id,
                            current_command_event_id, create_idempotency_key)
                           VALUES (?, ?, ?, ?, ?, ?, 'scheduled', 1, ?, ?, ?)"#,
                        [
                            item_id.as_str().into(),
                            account_id.into(),
                            kind.as_str().into(),
                            title.clone().into(),
                            (*scheduled_at_unix_secs).into(),
                            timezone.clone().into(),
                            request.command_source_event_id.as_str().into(),
                            request.command_source_event_id.as_str().into(),
                            request.idempotency_key.clone().into(),
                        ],
                    ))
                    .await
                    .map_err(map_db)?;
                let row =
                    load_by_idempotency(&transaction, account_id, &request.idempotency_key).await?;
                (map_row(row, request.account.clone())?, None)
            }
            AgendaMutation::Reschedule {
                item_id,
                expected_version,
                scheduled_at_unix_secs,
                timezone,
            }
            | AgendaMutation::Snooze {
                item_id,
                expected_version,
                scheduled_at_unix_secs,
                timezone,
            } => {
                update_item(
                    &transaction,
                    account_id,
                    item_id,
                    *expected_version,
                    &request.command_source_event_id,
                    "item_status = 'scheduled', scheduled_at_unix_secs = ?, timezone_name = ?",
                    vec![(*scheduled_at_unix_secs).into(), timezone.clone().into()],
                )
                .await?;
                suppress_old_notifications(&transaction, item_id, *expected_version).await?;
                (
                    map_row(
                        load_item(&transaction, account_id, item_id).await?,
                        request.account.clone(),
                    )?,
                    Some(*expected_version),
                )
            }
            AgendaMutation::Complete {
                item_id,
                expected_version,
            } => {
                update_item(
                    &transaction,
                    account_id,
                    item_id,
                    *expected_version,
                    &request.command_source_event_id,
                    "item_status = 'completed'",
                    Vec::new(),
                )
                .await?;
                suppress_old_notifications(&transaction, item_id, u64::MAX).await?;
                (
                    map_row(
                        load_item(&transaction, account_id, item_id).await?,
                        request.account.clone(),
                    )?,
                    Some(*expected_version),
                )
            }
            AgendaMutation::Cancel {
                item_id,
                expected_version,
            } => {
                update_item(
                    &transaction,
                    account_id,
                    item_id,
                    *expected_version,
                    &request.command_source_event_id,
                    "item_status = 'cancelled'",
                    Vec::new(),
                )
                .await?;
                suppress_old_notifications(&transaction, item_id, u64::MAX).await?;
                (
                    map_row(
                        load_item(&transaction, account_id, item_id).await?,
                        request.account.clone(),
                    )?,
                    Some(*expected_version),
                )
            }
        };

        let detail_json = serde_json::json!({
            "item_id": item.item_id.as_str(),
            "status": item.status.as_str(),
            "version": item.version,
            "scheduled_at_unix_secs": item.scheduled_at_unix_secs,
        })
        .to_string();
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_agenda_mutation_audit
                   (audit_id, item_id, account_id, command_source_event_id, run_id, effect_id,
                    mutation_kind, from_version, to_version, detail_json)
                   VALUES (UUID(), ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                [
                    item.item_id.as_str().into(),
                    account_id.into(),
                    request.command_source_event_id.as_str().into(),
                    request.run_id.clone().into(),
                    request.effect_id.clone().into(),
                    request.mutation.kind().into(),
                    from_version.into(),
                    item.version.into(),
                    detail_json.into(),
                ],
            ))
            .await
            .map_err(map_db)?;
        let result_ref = format!(
            "agenda:{}:v{}:{}:{}",
            item.item_id.as_str(),
            item.version,
            item.status.as_str(),
            item.scheduled_at_unix_secs
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".into())
        );
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_action_effect_receipts
                   (effect_id, run_id, proposal_json, result_ref)
                   VALUES (?, ?, ?, ?)"#,
                [
                    request.effect_id.clone().into(),
                    request.run_id.clone().into(),
                    request.proposal_json.clone().into(),
                    result_ref.clone().into(),
                ],
            ))
            .await
            .map_err(map_db)?;
        transaction.commit().await.map_err(map_db)?;
        Ok(AgendaMutationReceipt { item, result_ref })
    }

    async fn list_upcoming(
        &self,
        account: &SourceAccountRef,
        now_unix_secs: i64,
        horizon_secs: u64,
        limit: u32,
    ) -> Result<Vec<AgendaItem>, AgendaError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        let deadline = now_unix_secs.saturating_add(horizon_secs.min(i64::MAX as u64) as i64);
        let rows = AgendaRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT item_id, item_kind, title, scheduled_at_unix_secs, timezone_name,
                      item_status, version, created_command_event_id, current_command_event_id,
                      CAST(UNIX_TIMESTAMP(created_at) AS SIGNED) AS created_at_unix_secs,
                      CAST(UNIX_TIMESTAMP(updated_at) AS SIGNED) AS updated_at_unix_secs
               FROM secretary_agenda_items
               WHERE account_id = ? AND item_status = 'scheduled'
                 AND scheduled_at_unix_secs BETWEEN ? AND ?
               ORDER BY scheduled_at_unix_secs, item_id LIMIT ?"#,
            [
                account_id.into(),
                now_unix_secs.into(),
                deadline.into(),
                limit.into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(map_db)?;
        rows.into_iter()
            .map(|row| map_row(row, account.clone()))
            .collect()
    }

    async fn produce_due_notification_candidates(
        &self,
        now_unix_secs: i64,
        limit: u32,
    ) -> Result<NotificationCandidateProductionReport, AgendaError> {
        let transaction = self.db.begin().await.map_err(map_db)?;
        let items = DueAgendaItemRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT item.item_id, item.account_id, item.version, account.source_channel, \
                    account.platform_account_id \
             FROM secretary_agenda_items AS item \
             INNER JOIN secretary_accounts AS account ON account.id = item.account_id \
             WHERE item.item_status = 'scheduled' AND item.scheduled_at_unix_secs <= ? \
             ORDER BY item.scheduled_at_unix_secs, item.item_id LIMIT ? FOR UPDATE SKIP LOCKED",
            [now_unix_secs.into(), limit.into()],
        ))
        .all(&transaction)
        .await
        .map_err(map_db)?;
        let mut report = NotificationCandidateProductionReport::default();
        for item in items {
            let production = produce_from_locked_source(
                &transaction,
                &LockedNotificationSource::Agenda {
                    account_id: item.account_id,
                    item_id: item.item_id,
                    version: item.version,
                    source_channel: item.source_channel,
                    platform_account_id: item.platform_account_id,
                },
            )
            .await
            .map_err(|error| AgendaError::Store(error.to_string()))?;
            report.candidates_created += u64::from(production.candidate_created);
            report.requests_created += u64::from(production.request_created);
        }
        transaction.commit().await.map_err(map_db)?;
        Ok(report)
    }
}

async fn update_item<C: ConnectionTrait>(
    db: &C,
    account_id: u64,
    item_id: &AgendaItemId,
    expected_version: u64,
    command_event: &SourceEventId,
    assignments: &str,
    mut values: Vec<sea_orm::Value>,
) -> Result<(), AgendaError> {
    let sql = format!(
        "UPDATE secretary_agenda_items SET {assignments}, version = version + 1, current_command_event_id = ?, updated_at = UTC_TIMESTAMP(6) WHERE item_id = ? AND account_id = ? AND version = ? AND item_status = 'scheduled'"
    );
    values.extend([
        command_event.as_str().into(),
        item_id.as_str().into(),
        account_id.into(),
        expected_version.into(),
    ]);
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .await
        .map_err(map_db)?;
    if result.rows_affected() != 1 {
        return Err(AgendaError::VersionConflict);
    }
    Ok(())
}

async fn suppress_old_notifications<C: ConnectionTrait>(
    db: &C,
    item_id: &AgendaItemId,
    through_version: u64,
) -> Result<(), AgendaError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_notification_outbox SET delivery_status = 'suppressed', lease_token = NULL, lease_expires_at = NULL WHERE agenda_item_id = ? AND agenda_version <= ? AND delivery_status IN ('pending', 'failed')",
        [item_id.as_str().into(), through_version.into()],
    )).await.map_err(map_db)?;
    Ok(())
}

async fn resolve_account_id<C: ConnectionTrait>(
    db: &C,
    account: &SourceAccountRef,
) -> Result<u64, AgendaError> {
    AccountIdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ? AND status = 'active'",
        [account.channel.as_str().into(), account.account_id.clone().into()],
    )).one(db).await.map_err(map_db)?.map(|row| row.id).ok_or(AgendaError::NotFound)
}

async fn load_item<C: ConnectionTrait>(
    db: &C,
    account_id: u64,
    item_id: &AgendaItemId,
) -> Result<AgendaRow, AgendaError> {
    AgendaRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT item_id, item_kind, title, scheduled_at_unix_secs, timezone_name,
                  item_status, version, created_command_event_id, current_command_event_id,
                  CAST(UNIX_TIMESTAMP(created_at) AS SIGNED) AS created_at_unix_secs,
                  CAST(UNIX_TIMESTAMP(updated_at) AS SIGNED) AS updated_at_unix_secs
           FROM secretary_agenda_items WHERE account_id = ? AND item_id = ?"#,
        [account_id.into(), item_id.as_str().into()],
    ))
    .one(db)
    .await
    .map_err(map_db)?
    .ok_or(AgendaError::NotFound)
}

async fn load_by_idempotency<C: ConnectionTrait>(
    db: &C,
    account_id: u64,
    key: &str,
) -> Result<AgendaRow, AgendaError> {
    let item = AgendaItemId::new(
        IdRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT item_id AS value FROM secretary_agenda_items WHERE account_id = ? AND create_idempotency_key = ?",
            [account_id.into(), key.into()],
        )).one(db).await.map_err(map_db)?.ok_or(AgendaError::NotFound)?.value,
    )?;
    load_item(db, account_id, &item).await
}

fn map_row(row: AgendaRow, account: SourceAccountRef) -> Result<AgendaItem, AgendaError> {
    Ok(AgendaItem {
        item_id: AgendaItemId::new(row.item_id)?,
        account,
        kind: match row.item_kind.as_str() {
            "schedule" => AgendaItemKind::Schedule,
            "task" => AgendaItemKind::Task,
            "reminder" => AgendaItemKind::Reminder,
            _ => return Err(AgendaError::Store("invalid agenda kind".into())),
        },
        title: row.title,
        scheduled_at_unix_secs: row.scheduled_at_unix_secs,
        timezone: row.timezone_name,
        status: match row.item_status.as_str() {
            "scheduled" => AgendaItemStatus::Scheduled,
            "completed" => AgendaItemStatus::Completed,
            "cancelled" => AgendaItemStatus::Cancelled,
            _ => return Err(AgendaError::Store("invalid agenda status".into())),
        },
        version: row.version,
        created_by_command: SourceEventId::new(row.created_command_event_id)
            .map_err(|error| AgendaError::Store(error.to_string()))?,
        current_version_command: SourceEventId::new(row.current_command_event_id)
            .map_err(|error| AgendaError::Store(error.to_string()))?,
        created_at_unix_secs: row.created_at_unix_secs,
        updated_at_unix_secs: row.updated_at_unix_secs,
    })
}

fn map_db(error: sea_orm::DbErr) -> AgendaError {
    let mapped = store_error(error);
    AgendaError::Store(mapped.to_string())
}

#[derive(Debug, FromQueryResult)]
struct DueAgendaItemRow {
    item_id: String,
    account_id: u64,
    version: u64,
    source_channel: String,
    platform_account_id: String,
}

#[derive(Debug, FromQueryResult)]
struct AccountIdRow {
    id: u64,
}
#[derive(Debug, FromQueryResult)]
struct IdRow {
    value: String,
}
#[derive(Debug, FromQueryResult)]
struct ReceiptRow {
    result_ref: String,
}
#[derive(Debug, FromQueryResult)]
struct AgendaRow {
    item_id: String,
    item_kind: String,
    title: String,
    scheduled_at_unix_secs: Option<i64>,
    timezone_name: String,
    item_status: String,
    version: u64,
    created_command_event_id: String,
    current_command_event_id: String,
    created_at_unix_secs: i64,
    updated_at_unix_secs: i64,
}
