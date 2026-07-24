use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use tracing::{debug, info};

use crate::{
    ClaimedOwnerNotification, FollowUpScanReport, FollowUpStoreT, InboundEventStoreError,
    MemoryFact, MemoryPayload, MessageSource, NotificationFailureKind, NotificationId,
    NotificationLeaseToken, SourceAccountRef,
};

use super::mysql_inbound::store_error;

pub(crate) struct MySqlFollowUpStore {
    db: DatabaseConnection,
}

impl MySqlFollowUpStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl FollowUpStoreT for MySqlFollowUpStore {
    async fn scan_commitments(
        &self,
        now_unix_secs: i64,
        horizon_secs: i64,
        limit: u32,
    ) -> Result<FollowUpScanReport, InboundEventStoreError> {
        if !(60..=31_536_000).contains(&horizon_secs) || !(1..=1000).contains(&limit) {
            return Err(InboundEventStoreError::InvalidData(
                "follow-up scan bounds are invalid".into(),
            ));
        }
        let horizon = now_unix_secs.saturating_add(horizon_secs);
        let transaction = self.db.begin().await.map_err(store_error)?;

        let completed = transaction.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"UPDATE secretary_follow_up_items
               SET status = 'completed', updated_at = CURRENT_TIMESTAMP(6)
               WHERE follow_up_id IN (
                 SELECT follow_up_id FROM (
                   SELECT item.follow_up_id
                   FROM secretary_follow_up_items item
                   JOIN secretary_memory_facts fact ON fact.fact_id = item.source_memory_fact_id
                   WHERE item.status = 'scheduled'
                     AND JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.status')) = 'fulfilled'
                   ORDER BY item.updated_at, item.follow_up_id LIMIT ?
                 ) bounded
               )"#,
            [limit.into()],
        )).await.map_err(store_error)?.rows_affected();

        let superseded = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_follow_up_items
               SET status = 'superseded', updated_at = CURRENT_TIMESTAMP(6)
               WHERE follow_up_id IN (
                 SELECT follow_up_id FROM (
                   SELECT item.follow_up_id
                   FROM secretary_follow_up_items item
                   JOIN secretary_memory_facts fact ON fact.fact_id = item.source_memory_fact_id
                   WHERE item.status = 'scheduled'
                     AND (fact.fact_status <> 'confirmed'
                          OR JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.status')) = 'cancelled')
                   ORDER BY item.updated_at, item.follow_up_id LIMIT ?
                 ) bounded
               )"#,
                [limit.into()],
            ))
            .await
            .map_err(store_error)?
            .rows_affected();

        transaction
            .execute_raw(Statement::from_string(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_notification_outbox notification
                   JOIN secretary_follow_up_items item ON item.follow_up_id = notification.follow_up_id
                   SET notification.delivery_status = 'suppressed',
                       notification.lease_token = NULL, notification.lease_expires_at = NULL
                   WHERE notification.delivery_status IN ('pending', 'failed')
                     AND item.status <> 'scheduled'"#,
            ))
            .await
            .map_err(store_error)?;

        let materialized = transaction.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"INSERT IGNORE INTO secretary_follow_up_items
                 (follow_up_id, account_id, source_memory_fact_id, reason_code, due_at_unix_secs, status)
               SELECT UUID(), fact.account_id, fact.fact_id, 'commitment_due',
                      CAST(JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.due_at_unix_secs')) AS SIGNED),
                      'scheduled'
               FROM secretary_memory_facts fact
               WHERE fact.fact_kind = 'commitment' AND fact.fact_status = 'confirmed'
                 AND JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.status')) IN ('pending', 'proposed')
                 AND JSON_EXTRACT(fact.fact_json, '$.payload.data.due_at_unix_secs') IS NOT NULL
                 AND CAST(JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.due_at_unix_secs')) AS SIGNED) <= ?
               ORDER BY fact.updated_at, fact.fact_id LIMIT ?"#,
            [horizon.into(), limit.into()],
        )).await.map_err(store_error)?.rows_affected();

        let enqueued = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_notification_outbox
                 (notification_id, account_id, follow_up_id, scheduled_at_unix_secs,
                  notification_kind, payload_json, delivery_status)
               SELECT UUID(), item.account_id, item.follow_up_id, item.due_at_unix_secs,
                      'owner_reminder',
                      JSON_OBJECT('follow_up_id', item.follow_up_id,
                                  'source_memory_fact_id', item.source_memory_fact_id,
                                  'reason_code', item.reason_code),
                      'pending'
               FROM secretary_follow_up_items item
               WHERE item.status = 'scheduled' AND item.due_at_unix_secs <= ?
               ORDER BY item.due_at_unix_secs, item.follow_up_id LIMIT ?"#,
                [now_unix_secs.into(), limit.into()],
            ))
            .await
            .map_err(store_error)?
            .rows_affected();

        transaction.commit().await.map_err(store_error)?;
        let report = FollowUpScanReport {
            commitments_materialized: materialized,
            items_reconciled: completed.saturating_add(superseded),
            notifications_enqueued: enqueued,
            memories_expired: 0,
        };
        if materialized > 0 || completed > 0 || superseded > 0 || enqueued > 0 {
            info!(
                commitments_materialized = report.commitments_materialized,
                items_reconciled = report.items_reconciled,
                notifications_enqueued = report.notifications_enqueued,
                "follow-up scheduler persisted a bounded scan"
            );
        } else {
            debug!("follow-up scheduler scan had no changes");
        }
        Ok(report)
    }

    async fn claim_due_notification(
        &self,
        account: &SourceAccountRef,
        now_unix_secs: i64,
        lease_secs: u64,
    ) -> Result<Option<ClaimedOwnerNotification>, InboundEventStoreError> {
        if !(1..=3600).contains(&lease_secs) {
            return Err(InboundEventStoreError::InvalidData(
                "notification lease_secs must be in 1..=3600".into(),
            ));
        }
        let transaction = self.db.begin().await.map_err(store_error)?;
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_notification_outbox notification
                   JOIN secretary_accounts account ON account.id = notification.account_id
                   SET notification.delivery_status = 'unknown_commit',
                       notification.last_error_code = 'lease_expired_in_flight',
                       notification.lease_token = NULL, notification.lease_expires_at = NULL
                   WHERE notification.delivery_status = 'claimed'
                     AND notification.lease_expires_at < UTC_TIMESTAMP(6)
                     AND account.source_channel = ? AND account.platform_account_id = ?"#,
                [
                    account.channel.as_str().into(),
                    account.account_id.clone().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        let Some(row) = NotificationClaimRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT notification.notification_id, notification.attempts,
                      account.source_channel, account.platform_account_id,
                      item.due_at_unix_secs, CAST(fact.fact_json AS CHAR) AS fact_json
               FROM secretary_notification_outbox notification
               JOIN secretary_follow_up_items item ON item.follow_up_id = notification.follow_up_id
               JOIN secretary_memory_facts fact ON fact.fact_id = item.source_memory_fact_id
               JOIN secretary_accounts account ON account.id = notification.account_id
               WHERE notification.delivery_status = 'pending'
                 AND notification.scheduled_at_unix_secs <= ?
                 AND item.status = 'scheduled' AND fact.fact_status = 'confirmed'
                 AND account.source_channel = ? AND account.platform_account_id = ?
               ORDER BY notification.scheduled_at_unix_secs, notification.notification_id
               LIMIT 1 FOR UPDATE SKIP LOCKED"#,
            [
                now_unix_secs.into(),
                account.channel.as_str().into(),
                account.account_id.clone().into(),
            ],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        else {
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        };
        let lease_token = NotificationLeaseToken::generate();
        let updated = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_notification_outbox
                   SET delivery_status = 'claimed', attempts = attempts + 1,
                       lease_token = ?, lease_expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND)
                   WHERE notification_id = ? AND delivery_status = 'pending'"#,
                [
                    lease_token.as_str().into(),
                    lease_secs.into(),
                    row.notification_id.clone().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if updated.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        transaction.commit().await.map_err(store_error)?;
        let fact: MemoryFact = serde_json::from_str(&row.fact_json).map_err(|error| {
            InboundEventStoreError::InvalidData(format!(
                "notification source memory is invalid: {error}"
            ))
        })?;
        let MemoryPayload::Commitment(commitment) = fact.payload else {
            return Err(InboundEventStoreError::InvalidData(
                "notification source is not a commitment".into(),
            ));
        };
        Ok(Some(ClaimedOwnerNotification {
            notification_id: NotificationId::new(row.notification_id)
                .map_err(InboundEventStoreError::InvalidData)?,
            lease_token,
            managed_account: SourceAccountRef::new(
                parse_source(&row.source_channel)?,
                row.platform_account_id,
            )
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
            commitment,
            due_at_unix_secs: row.due_at_unix_secs,
            attempt: row.attempts.saturating_add(1),
        }))
    }

    async fn mark_notification_delivered(
        &self,
        notification_id: &NotificationId,
        lease_token: &NotificationLeaseToken,
        platform_message_id: &str,
    ) -> Result<(), InboundEventStoreError> {
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_notification_outbox
               SET delivery_status = 'delivered', platform_message_id = ?,
                   delivered_at = UTC_TIMESTAMP(6), lease_token = NULL, lease_expires_at = NULL
               WHERE notification_id = ? AND delivery_status = 'claimed' AND lease_token = ?"#,
                [
                    platform_message_id.into(),
                    notification_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        info!(
            notification_id = notification_id.as_str(),
            "owner notification marked delivered"
        );
        Ok(())
    }

    async fn mark_notification_failed(
        &self,
        notification_id: &NotificationId,
        lease_token: &NotificationLeaseToken,
        error_code: &str,
        kind: NotificationFailureKind,
    ) -> Result<(), InboundEventStoreError> {
        let (status, retry) = match kind {
            NotificationFailureKind::Retryable => ("pending", true),
            NotificationFailureKind::Permanent => ("failed", false),
            NotificationFailureKind::UnknownCommit => ("unknown_commit", false),
        };
        let sql = if retry {
            r#"UPDATE secretary_notification_outbox
               SET delivery_status = ?, last_error_code = ?,
                   scheduled_at_unix_secs = UNIX_TIMESTAMP(UTC_TIMESTAMP())
                     + LEAST(3600, 30 * POW(2, LEAST(attempts, 7) - 1)),
                   lease_token = NULL, lease_expires_at = NULL
               WHERE notification_id = ? AND delivery_status = 'claimed' AND lease_token = ?"#
        } else {
            r#"UPDATE secretary_notification_outbox
               SET delivery_status = ?, last_error_code = ?, lease_token = NULL, lease_expires_at = NULL
               WHERE notification_id = ? AND delivery_status = 'claimed' AND lease_token = ?"#
        };
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                sql,
                [
                    status.into(),
                    error_code.into(),
                    notification_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        tracing::warn!(
            notification_id = notification_id.as_str(),
            status,
            error_code,
            "owner notification delivery failed"
        );
        Ok(())
    }
}

#[derive(Debug, FromQueryResult)]
struct NotificationClaimRow {
    notification_id: String,
    attempts: u32,
    source_channel: String,
    platform_account_id: String,
    due_at_unix_secs: i64,
    fact_json: String,
}

fn parse_source(value: &str) -> Result<MessageSource, InboundEventStoreError> {
    match value {
        "napcat" => Ok(MessageSource::NapCat),
        "qq_open_platform" => Ok(MessageSource::QqOpenPlatform),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown message source: {value}"
        ))),
    }
}
