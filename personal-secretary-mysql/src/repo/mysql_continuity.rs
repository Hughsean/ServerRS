use async_trait::async_trait;
use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, IntoActiveModel,
    QueryFilter, Set, Statement, TransactionTrait,
};
use uuid::Uuid;

use crate::{
    ConnectionEndReason, ConnectionEpochId, ConnectionEpochStatus, ConversationRef,
    InboundEventStoreError, IngestionContinuityStoreT, IngestionGapId, IngestionGapReason,
    IngestionGapStatus, SourceAccountRef,
};

use super::MySqlInboundEventStore;
use super::entities::{secretary_connection_epochs, secretary_ingestion_gaps};
use super::mysql_inbound::{ensure_account_ref, ensure_conversation_ref, store_error};

/// `secretary_gap_boundaries` 要求非空父锚点。此值只表示“没有入站游标可冻结”，不会传给
/// NapCat 作为分页 cursor；回补会从最新页开始，并保持 `UnprovenStop` 直到有真实终点证据。
pub(super) const NO_PRIOR_CURSOR_BOUNDARY: &str = "__non_message_history_signal_no_prior_cursor__";

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
        tracing::info!("接入连接周期已标记为 connected");
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

        let created = insert_gap_if_absent(
            &transaction,
            account_id,
            connection_epoch_id,
            reason.as_str(),
            now,
        )
        .await?;
        let gap = gap_for_epoch(&transaction, connection_epoch_id).await?;
        // 只在 Gap 首次创建的事务冻结证据。即使当时没有游标或目录快照，重复调用也不能
        // 把后续数据补入旧 Gap。
        if created && let Some(gap_id) = gap.as_ref() {
            snapshot_gap_boundaries(&transaction, gap_id.as_str(), account_id, now).await?;
            freeze_directory_snapshot_for_gap(&transaction, gap_id.as_str(), account_id).await?;
        }
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
        mark_connection_uncertain_in_txn(self, connection_epoch_id, reason, None).await
    }

    async fn mark_connection_uncertain_for_conversation(
        &self,
        connection_epoch_id: &ConnectionEpochId,
        reason: IngestionGapReason,
        conversation: &ConversationRef,
    ) -> Result<IngestionGapId, InboundEventStoreError> {
        mark_connection_uncertain_in_txn(self, connection_epoch_id, reason, Some(conversation))
            .await
    }
}

async fn mark_connection_uncertain_in_txn(
    store: &MySqlInboundEventStore,
    connection_epoch_id: &ConnectionEpochId,
    reason: IngestionGapReason,
    conversation: Option<&ConversationRef>,
) -> Result<IngestionGapId, InboundEventStoreError> {
    if conversation.is_some() && reason != IngestionGapReason::NonMessageReference {
        return Err(InboundEventStoreError::InvalidData(
            "conversation-scoped uncertainty requires a non-message reference".into(),
        ));
    }
    let transaction = store.db.begin().await.map_err(store_error)?;
    let now = Utc::now().naive_utc();
    let epoch =
        secretary_connection_epochs::Entity::find_by_id(connection_epoch_id.as_str().to_owned())
            .one(&transaction)
            .await
            .map_err(store_error)?
            .filter(|epoch| epoch.status == ConnectionEpochStatus::Connected.as_str())
            .ok_or_else(|| {
                InboundEventStoreError::InvalidData(
                    "only a connected epoch can be marked uncertain".into(),
                )
            })?;

    let created = insert_gap_if_absent(
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
    if created {
        snapshot_gap_boundaries(&transaction, gap.as_str(), epoch.account_id, now).await?;
        freeze_directory_snapshot_for_gap(&transaction, gap.as_str(), epoch.account_id).await?;
    }
    if let Some(conversation) = conversation {
        let conversation_id =
            ensure_conversation_ref(&transaction, epoch.account_id, conversation, now).await?;
        // 已有游标边界优先：首次创建 Gap 时的 snapshot 会先写入真实锚点；只有未见过的
        // 会话才写入此 sentinel，使其从最新页开始有界回补。
        transaction
            .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "INSERT INTO secretary_gap_boundaries \
                     (gap_id, account_id, conversation_id, conversation_kind, platform_conversation_id, \
                      boundary_message_id, boundary_occurred_at_unix_secs, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?) \
                     ON DUPLICATE KEY UPDATE gap_id = gap_id",
                    [
                        gap.as_str().into(),
                        epoch.account_id.into(),
                        conversation_id.into(),
                        conversation.kind.as_str().into(),
                        conversation.id.clone().into(),
                        NO_PRIOR_CURSOR_BOUNDARY.into(),
                        now.into(),
                        now.into(),
                    ],
                ))
            .await
            .map_err(store_error)?;
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO secretary_gap_signal_scopes \
                 (gap_id, conversation_id, signal_kind, created_at, updated_at) \
                 VALUES (?, ?, 'non_message_reference', ?, ?) \
                 ON DUPLICATE KEY UPDATE gap_id = gap_id",
                [
                    gap.as_str().into(),
                    conversation_id.into(),
                    now.into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;
    }
    transaction.commit().await.map_err(store_error)?;
    tracing::warn!(reason = reason.as_str(), "连接周期已标记为消息连续性不确定");
    Ok(gap)
}

pub(super) async fn insert_gap_if_absent(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    connection_epoch_id: &ConnectionEpochId,
    reason: &str,
    now: chrono::NaiveDateTime,
) -> Result<bool, InboundEventStoreError> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"INSERT INTO secretary_ingestion_gaps
               (gap_id, account_id, connection_epoch_id, gap_started_at, gap_ended_at,
                status, reason, created_at, updated_at)
               VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?)
               ON DUPLICATE KEY UPDATE connection_epoch_id = connection_epoch_id"#,
            [
                Uuid::new_v4().to_string().into(),
                account_id.into(),
                connection_epoch_id.as_str().into(),
                now.into(),
                IngestionGapStatus::Uncertain.as_str().into(),
                reason.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await
        .map_err(store_error)?;
    Ok(result.rows_affected() == 1)
}

pub(super) async fn gap_for_epoch(
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

/// 在 Gap 创建事务内，把账号下所有会话级游标快照到 `secretary_gap_boundaries`。
///
/// 首写获胜：`ON DUPLICATE KEY UPDATE gap_id = gap_id` 不更新已存在的行，确保边界冻结在
/// 最早连续性中断点。回补时 `known_scopes_for_gap` 读取此快照，而非领取时漂移的实时游标。
pub(super) async fn snapshot_gap_boundaries(
    db: &sea_orm::DatabaseTransaction,
    gap_id: &str,
    account_id: u64,
    now: chrono::NaiveDateTime,
) -> Result<(), InboundEventStoreError> {
    let values: Vec<sea_orm::Value> =
        vec![gap_id.into(), now.into(), now.into(), account_id.into()];
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_gap_boundaries \
            (gap_id, account_id, conversation_id, conversation_kind, platform_conversation_id, \
             boundary_message_id, boundary_occurred_at_unix_secs, created_at, updated_at) \
         SELECT ?, cur.account_id, cur.conversation_id, c.conversation_kind, \
                c.platform_conversation_id, cur.last_platform_event_id, \
                cur.last_occurred_at_unix_secs, ?, ? \
         FROM secretary_ingestion_cursors cur \
         INNER JOIN secretary_conversations c ON c.id = cur.conversation_id \
         WHERE cur.account_id = ? AND cur.scope_kind = 'conversation' \
         ON DUPLICATE KEY UPDATE gap_id = gap_id",
        values,
    ))
    .await
    .map_err(store_error)?;
    Ok(())
}

/// 冻结账号最新目录快照到 Gap。`snapshot_id = NULL` 明确表示创建时没有目录快照。
///
/// 在 Gap 创建事务内执行，保证与 gap 行同事务可见。
pub(super) async fn freeze_directory_snapshot_for_gap(
    db: &sea_orm::DatabaseTransaction,
    gap_id: &str,
    account_id: u64,
) -> Result<(), InboundEventStoreError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"INSERT INTO secretary_directory_gap_freeze (gap_id, snapshot_id, account_id)
           VALUES (?, (
               SELECT s.snapshot_id
               FROM secretary_directory_snapshots s
               WHERE s.account_id = ?
               ORDER BY s.created_at_unix_secs DESC, s.snapshot_id DESC
               LIMIT 1
           ), ?)
           ON DUPLICATE KEY UPDATE gap_id = gap_id"#,
        [gap_id.into(), account_id.into(), account_id.into()],
    ))
    .await
    .map_err(store_error)?;
    Ok(())
}
