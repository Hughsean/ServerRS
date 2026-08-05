use async_trait::async_trait;
use chrono::{Duration, Utc};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, FromQueryResult, QueryFilter,
    Statement, TransactionTrait,
};
use uuid::Uuid;

use crate::{
    ClaimedLegacyRealtimeSpoolEpoch, ConnectionEpochStatus, InboundEventStoreError, IngestionGapId,
    LegacyRealtimeSpoolEpoch, RealtimeSpoolRecoveryLeaseToken, RealtimeSpoolRecoveryStoreT,
    SourceAccountRef,
};

use super::MySqlInboundEventStore;
use super::entities::{secretary_accounts, secretary_connection_epochs};
use super::mysql_continuity::{
    freeze_directory_snapshot_for_gap, gap_for_epoch, insert_gap_if_absent, snapshot_gap_boundaries,
};
use super::mysql_inbound::store_error;

const CLAIM_LIMIT: u64 = 100;

#[derive(Debug, FromQueryResult)]
struct ClaimableEpochRow {
    connection_epoch_id: String,
    account_id: u64,
    source_channel: String,
    status: String,
}

#[derive(Debug, FromQueryResult)]
struct VerifiedClaimRow {
    account_id: u64,
    source_channel: String,
    platform_account_id: String,
    status: String,
}

#[async_trait]
impl RealtimeSpoolRecoveryStoreT for MySqlInboundEventStore {
    async fn claim_legacy_realtime_spool_epochs(
        &self,
        account: &SourceAccountRef,
    ) -> Result<Vec<ClaimedLegacyRealtimeSpoolEpoch>, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let account_row = secretary_accounts::Entity::find()
            .filter(secretary_accounts::Column::SourceChannel.eq(account.channel.as_str()))
            .filter(secretary_accounts::Column::PlatformAccountId.eq(&account.account_id))
            .one(&transaction)
            .await
            .map_err(store_error)?;
        let Some(account_row) = account_row else {
            transaction.commit().await.map_err(store_error)?;
            return Ok(Vec::new());
        };
        let rows = ClaimableEpochRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT e.connection_epoch_id, e.account_id, e.source_channel, e.status
               FROM secretary_connection_epochs e
               LEFT JOIN secretary_realtime_spool_recovery_claims c
                 ON c.connection_epoch_id = e.connection_epoch_id
               WHERE e.account_id = ?
                 AND e.source_channel = ?
                 AND e.ended_at IS NULL
                 AND e.status IN ('connecting', 'connected')
                 AND (c.lease_token IS NULL OR c.lease_expires_at < NOW(6))
               ORDER BY e.started_at, e.connection_epoch_id
               LIMIT ?
               FOR UPDATE SKIP LOCKED"#,
            [
                account_row.id.into(),
                account.channel.as_str().into(),
                CLAIM_LIMIT.into(),
            ],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?;

        let now = Utc::now().naive_utc();
        let lease_millis = self
            .realtime_spool_recovery_lease_secs
            .saturating_mul(1_000)
            .min(i64::MAX as u64) as i64;
        let lease_expires_at = now
            .checked_add_signed(Duration::milliseconds(lease_millis))
            .ok_or_else(|| InboundEventStoreError::InvalidData("spool lease overflow".into()))?;
        let mut claimed = Vec::with_capacity(rows.len());
        for row in rows {
            if row.account_id != account_row.id || row.source_channel != account.channel.as_str() {
                return Err(InboundEventStoreError::InvalidData(
                    "spool claim account mismatch".into(),
                ));
            }
            let status = parse_status(&row.status)?;
            let lease_token = RealtimeSpoolRecoveryLeaseToken::new(Uuid::new_v4().to_string())
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"INSERT INTO secretary_realtime_spool_recovery_claims
                       (connection_epoch_id, account_id, lease_token, lease_expires_at,
                        created_at, updated_at)
                       VALUES (?, ?, ?, ?, ?, ?)
                       ON DUPLICATE KEY UPDATE
                         account_id = VALUES(account_id),
                         lease_token = VALUES(lease_token),
                         lease_expires_at = VALUES(lease_expires_at),
                         updated_at = VALUES(updated_at)"#,
                    [
                        row.connection_epoch_id.clone().into(),
                        row.account_id.into(),
                        lease_token.as_str().into(),
                        lease_expires_at.into(),
                        now.into(),
                        now.into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
            claimed.push(ClaimedLegacyRealtimeSpoolEpoch::new(
                LegacyRealtimeSpoolEpoch {
                    connection_epoch_id: crate::ConnectionEpochId::new(row.connection_epoch_id)
                        .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                    account: account.clone(),
                    status,
                },
                lease_token,
            ));
        }
        transaction.commit().await.map_err(store_error)?;
        Ok(claimed)
    }

    async fn finish_legacy_connecting_without_frames(
        &self,
        claimed: &ClaimedLegacyRealtimeSpoolEpoch,
    ) -> Result<(), InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let verified = verify_claim(&transaction, claimed).await?;
        if verified.status != ConnectionEpochStatus::Connecting.as_str() {
            return Err(InboundEventStoreError::InvalidData(
                "spool connecting recovery status mismatch".into(),
            ));
        }
        let now = Utc::now().naive_utc();
        let result = secretary_connection_epochs::Entity::update_many()
            .col_expr(
                secretary_connection_epochs::Column::Status,
                sea_orm::sea_query::Expr::value(ConnectionEpochStatus::ConnectFailed.as_str()),
            )
            .col_expr(
                secretary_connection_epochs::Column::EndedAt,
                sea_orm::sea_query::Expr::value(now),
            )
            .col_expr(
                secretary_connection_epochs::Column::EndReason,
                sea_orm::sea_query::Expr::value("spool_recovery_connect_failed"),
            )
            .col_expr(
                secretary_connection_epochs::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(now),
            )
            .filter(
                secretary_connection_epochs::Column::ConnectionEpochId
                    .eq(claimed.epoch().connection_epoch_id.as_str()),
            )
            .filter(secretary_connection_epochs::Column::EndedAt.is_null())
            .filter(
                secretary_connection_epochs::Column::Status
                    .eq(ConnectionEpochStatus::Connecting.as_str()),
            )
            .exec(&transaction)
            .await
            .map_err(store_error)?;
        if result.rows_affected != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        delete_claim(&transaction, claimed).await?;
        transaction.commit().await.map_err(store_error)
    }

    async fn renew_legacy_realtime_spool_epoch(
        &self,
        claimed: &ClaimedLegacyRealtimeSpoolEpoch,
    ) -> Result<(), InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        verify_claim(&transaction, claimed).await?;
        let lease_millis = self
            .realtime_spool_recovery_lease_secs
            .saturating_mul(1_000)
            .min(i64::MAX as u64) as i64;
        let lease_expires_at = Utc::now()
            .naive_utc()
            .checked_add_signed(Duration::milliseconds(lease_millis))
            .ok_or_else(|| InboundEventStoreError::InvalidData("spool lease overflow".into()))?;
        let result = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_realtime_spool_recovery_claims
                   SET lease_expires_at = ?, updated_at = NOW(6)
                   WHERE connection_epoch_id = ?
                     AND lease_token = ?
                     AND lease_expires_at >= NOW(6)"#,
                [
                    lease_expires_at.into(),
                    claimed.epoch().connection_epoch_id.as_str().into(),
                    claimed.lease_token().as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        transaction.commit().await.map_err(store_error)
    }

    async fn finalize_recovered_connected_epoch(
        &self,
        claimed: &ClaimedLegacyRealtimeSpoolEpoch,
    ) -> Result<IngestionGapId, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let verified = verify_claim(&transaction, claimed).await?;
        if verified.status != ConnectionEpochStatus::Connected.as_str() {
            return Err(InboundEventStoreError::InvalidData(
                "spool connected recovery status mismatch".into(),
            ));
        }
        let now = Utc::now().naive_utc();
        let result = secretary_connection_epochs::Entity::update_many()
            .col_expr(
                secretary_connection_epochs::Column::Status,
                sea_orm::sea_query::Expr::value(ConnectionEpochStatus::Disconnected.as_str()),
            )
            .col_expr(
                secretary_connection_epochs::Column::EndedAt,
                sea_orm::sea_query::Expr::value(now),
            )
            .col_expr(
                secretary_connection_epochs::Column::EndReason,
                sea_orm::sea_query::Expr::value("spool_recovery"),
            )
            .col_expr(
                secretary_connection_epochs::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(now),
            )
            .filter(
                secretary_connection_epochs::Column::ConnectionEpochId
                    .eq(claimed.epoch().connection_epoch_id.as_str()),
            )
            .filter(secretary_connection_epochs::Column::EndedAt.is_null())
            .filter(
                secretary_connection_epochs::Column::Status
                    .eq(ConnectionEpochStatus::Connected.as_str()),
            )
            .exec(&transaction)
            .await
            .map_err(store_error)?;
        if result.rows_affected != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        let created = insert_gap_if_absent(
            &transaction,
            verified.account_id,
            &claimed.epoch().connection_epoch_id,
            "spool_recovery",
            now,
        )
        .await?;
        let gap = gap_for_epoch(&transaction, &claimed.epoch().connection_epoch_id)
            .await?
            .ok_or(InboundEventStoreError::Unavailable)?;
        if created {
            snapshot_gap_boundaries(&transaction, gap.as_str(), verified.account_id, now).await?;
            freeze_directory_snapshot_for_gap(&transaction, gap.as_str(), verified.account_id)
                .await?;
        }
        delete_claim(&transaction, claimed).await?;
        transaction.commit().await.map_err(store_error)?;
        Ok(gap)
    }
}

async fn verify_claim(
    transaction: &sea_orm::DatabaseTransaction,
    claimed: &ClaimedLegacyRealtimeSpoolEpoch,
) -> Result<VerifiedClaimRow, InboundEventStoreError> {
    let row = VerifiedClaimRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT e.account_id, e.source_channel, a.platform_account_id, e.status
           FROM secretary_realtime_spool_recovery_claims c
           INNER JOIN secretary_connection_epochs e
             ON e.connection_epoch_id = c.connection_epoch_id
           INNER JOIN secretary_accounts a ON a.id = e.account_id
           WHERE c.connection_epoch_id = ?
             AND c.lease_token = ?
             AND c.lease_expires_at >= NOW(6)
             AND e.ended_at IS NULL
           FOR UPDATE"#,
        [
            claimed.epoch().connection_epoch_id.as_str().into(),
            claimed.lease_token().as_str().into(),
        ],
    ))
    .one(transaction)
    .await
    .map_err(store_error)?
    .ok_or(InboundEventStoreError::LeaseLost)?;
    if row.source_channel != claimed.epoch().account.channel.as_str()
        || row.platform_account_id != claimed.epoch().account.account_id
    {
        return Err(InboundEventStoreError::LeaseLost);
    }
    Ok(row)
}

async fn delete_claim(
    transaction: &sea_orm::DatabaseTransaction,
    claimed: &ClaimedLegacyRealtimeSpoolEpoch,
) -> Result<(), InboundEventStoreError> {
    let result = transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"DELETE FROM secretary_realtime_spool_recovery_claims
               WHERE connection_epoch_id = ?
                 AND lease_token = ?
                 AND lease_expires_at >= NOW(6)"#,
            [
                claimed.epoch().connection_epoch_id.as_str().into(),
                claimed.lease_token().as_str().into(),
            ],
        ))
        .await
        .map_err(store_error)?;
    if result.rows_affected() != 1 {
        return Err(InboundEventStoreError::LeaseLost);
    }
    Ok(())
}

fn parse_status(value: &str) -> Result<ConnectionEpochStatus, InboundEventStoreError> {
    match value {
        "connecting" => Ok(ConnectionEpochStatus::Connecting),
        "connected" => Ok(ConnectionEpochStatus::Connected),
        _ => Err(InboundEventStoreError::InvalidData(
            "invalid legacy spool epoch status".into(),
        )),
    }
}
