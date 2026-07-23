use async_trait::async_trait;
use chrono::Utc;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::{
    ConnectionEndReason, ConnectionEpochId, ConnectionEpochStatus, InboundEventStoreError,
    IngestionContinuityStoreT, IngestionGapId, IngestionGapReason, IngestionGapStatus,
    SourceAccountRef,
};

use super::MySqlInboundEventStore;
use super::entities::{secretary_connection_epochs, secretary_ingestion_gaps};
use super::mysql_inbound::{ensure_account_ref, store_error};

#[async_trait]
impl IngestionContinuityStoreT for MySqlInboundEventStore {
    async fn begin_connection(
        &self,
        account: &SourceAccountRef,
    ) -> Result<ConnectionEpochId, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let account_id = ensure_account_ref(&transaction, account, now).await?;
        let connection_epoch_id = ConnectionEpochId::new(Uuid::new_v4().to_string())
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;

        secretary_connection_epochs::Entity::insert(secretary_connection_epochs::ActiveModel {
            connection_epoch_id: Set(connection_epoch_id.as_str().to_owned()),
            account_id: Set(account_id),
            source_channel: Set(account.channel.as_str().into()),
            status: Set(ConnectionEpochStatus::Connecting.as_str().into()),
            started_at: Set(now),
            connected_at: Set(None),
            ended_at: Set(None),
            last_event_at: Set(None),
            last_source_event_id: Set(None),
            end_reason: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .exec(&transaction)
        .await
        .map_err(store_error)?;
        transaction.commit().await.map_err(store_error)?;
        tracing::debug!(
            connection_epoch_id = %connection_epoch_id.as_str(),
            source = account.channel.as_str(),
            source_account_id = %account.account_id,
            "已创建接入连接周期"
        );
        Ok(connection_epoch_id)
    }

    async fn mark_connection_connected(
        &self,
        connection_epoch_id: &ConnectionEpochId,
    ) -> Result<(), InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let epoch = secretary_connection_epochs::Entity::find_by_id(
            connection_epoch_id.as_str().to_owned(),
        )
        .one(&transaction)
        .await
        .map_err(store_error)?
        .ok_or_else(|| InboundEventStoreError::InvalidData("connection epoch not found".into()))?;
        if epoch.ended_at.is_some() {
            return Err(InboundEventStoreError::InvalidData(
                "ended connection epoch cannot become connected".into(),
            ));
        }

        let account_id = epoch.account_id;
        let mut active = epoch.into_active_model();
        active.status = Set(ConnectionEpochStatus::Connected.as_str().into());
        active.connected_at = Set(Some(now));
        active.updated_at = Set(now);
        active.update(&transaction).await.map_err(store_error)?;

        secretary_ingestion_gaps::Entity::update_many()
            .col_expr(
                secretary_ingestion_gaps::Column::GapEndedAt,
                Expr::value(now),
            )
            .col_expr(
                secretary_ingestion_gaps::Column::UpdatedAt,
                Expr::value(now),
            )
            .filter(secretary_ingestion_gaps::Column::AccountId.eq(account_id))
            .filter(secretary_ingestion_gaps::Column::GapEndedAt.is_null())
            .exec(&transaction)
            .await
            .map_err(store_error)?;

        transaction.commit().await.map_err(store_error)?;
        tracing::info!(
            connection_epoch_id = %connection_epoch_id.as_str(),
            "接入连接周期已标记为 connected"
        );
        Ok(())
    }

    async fn finish_connection(
        &self,
        connection_epoch_id: &ConnectionEpochId,
        reason: ConnectionEndReason,
    ) -> Result<Option<IngestionGapId>, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let epoch = secretary_connection_epochs::Entity::find_by_id(
            connection_epoch_id.as_str().to_owned(),
        )
        .one(&transaction)
        .await
        .map_err(store_error)?
        .ok_or_else(|| InboundEventStoreError::InvalidData("connection epoch not found".into()))?;

        if epoch.ended_at.is_some() {
            let gap = gap_for_epoch(&transaction, connection_epoch_id).await?;
            transaction.commit().await.map_err(store_error)?;
            return Ok(gap);
        }

        let was_connected = epoch.connected_at.is_some();
        let account_id = epoch.account_id;
        let status = if !was_connected {
            ConnectionEpochStatus::ConnectFailed.as_str()
        } else if reason == ConnectionEndReason::ProcessShutdown {
            ConnectionEpochStatus::Shutdown.as_str()
        } else {
            ConnectionEpochStatus::Disconnected.as_str()
        };
        let mut active = epoch.into_active_model();
        active.status = Set(status.into());
        active.ended_at = Set(Some(now));
        active.end_reason = Set(Some(reason.as_str().into()));
        active.updated_at = Set(now);
        active.update(&transaction).await.map_err(store_error)?;

        if !was_connected {
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        }

        let proposed_gap_id = Uuid::new_v4().to_string();
        secretary_ingestion_gaps::Entity::insert(secretary_ingestion_gaps::ActiveModel {
            gap_id: Set(proposed_gap_id),
            account_id: Set(account_id),
            connection_epoch_id: Set(connection_epoch_id.as_str().to_owned()),
            gap_started_at: Set(now),
            gap_ended_at: Set(None),
            status: Set(IngestionGapStatus::Uncertain.as_str().into()),
            reason: Set(reason.as_str().into()),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .on_conflict(
            OnConflict::column(secretary_ingestion_gaps::Column::ConnectionEpochId)
                .update_column(secretary_ingestion_gaps::Column::ConnectionEpochId)
                .to_owned(),
        )
        .exec(&transaction)
        .await
        .map_err(store_error)?;
        let gap = gap_for_epoch(&transaction, connection_epoch_id).await?;
        transaction.commit().await.map_err(store_error)?;
        tracing::debug!(
            connection_epoch_id = %connection_epoch_id.as_str(),
            status,
            reason = reason.as_str(),
            gap_id = gap.as_ref().map(IngestionGapId::as_str),
            "接入连接周期已结束"
        );
        Ok(gap)
    }

    async fn mark_connection_uncertain(
        &self,
        connection_epoch_id: &ConnectionEpochId,
        reason: IngestionGapReason,
    ) -> Result<IngestionGapId, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let epoch = secretary_connection_epochs::Entity::find_by_id(
            connection_epoch_id.as_str().to_owned(),
        )
        .one(&transaction)
        .await
        .map_err(store_error)?
        .filter(|epoch| epoch.status == ConnectionEpochStatus::Connected.as_str())
        .ok_or_else(|| {
            InboundEventStoreError::InvalidData(
                "only a connected epoch can be marked uncertain".into(),
            )
        })?;

        insert_gap_if_absent(
            &transaction,
            epoch.account_id,
            connection_epoch_id,
            reason.as_str(),
            now,
        )
        .await?;
        let gap = gap_for_epoch(&transaction, connection_epoch_id)
            .await?
            .ok_or(InboundEventStoreError::Unavailable)?;
        transaction.commit().await.map_err(store_error)?;
        tracing::warn!(
            connection_epoch_id = %connection_epoch_id.as_str(),
            gap_id = %gap.as_str(),
            reason = reason.as_str(),
            "连接周期已标记为消息连续性不确定"
        );
        Ok(gap)
    }
}

async fn insert_gap_if_absent(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    connection_epoch_id: &ConnectionEpochId,
    reason: &str,
    now: chrono::NaiveDateTime,
) -> Result<(), InboundEventStoreError> {
    secretary_ingestion_gaps::Entity::insert(secretary_ingestion_gaps::ActiveModel {
        gap_id: Set(Uuid::new_v4().to_string()),
        account_id: Set(account_id),
        connection_epoch_id: Set(connection_epoch_id.as_str().to_owned()),
        gap_started_at: Set(now),
        gap_ended_at: Set(None),
        status: Set(IngestionGapStatus::Uncertain.as_str().into()),
        reason: Set(reason.into()),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::column(secretary_ingestion_gaps::Column::ConnectionEpochId)
            .update_column(secretary_ingestion_gaps::Column::ConnectionEpochId)
            .to_owned(),
    )
    .exec(db)
    .await
    .map_err(store_error)?;
    Ok(())
}

async fn gap_for_epoch(
    db: &sea_orm::DatabaseTransaction,
    connection_epoch_id: &ConnectionEpochId,
) -> Result<Option<IngestionGapId>, InboundEventStoreError> {
    secretary_ingestion_gaps::Entity::find()
        .filter(
            secretary_ingestion_gaps::Column::ConnectionEpochId.eq(connection_epoch_id.as_str()),
        )
        .one(db)
        .await
        .map_err(store_error)?
        .map(|gap| {
            IngestionGapId::new(gap.gap_id)
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))
        })
        .transpose()
}
