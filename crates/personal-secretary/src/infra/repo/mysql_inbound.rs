use async_trait::async_trait;
use chrono::Utc;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, Set, TransactionTrait,
};
use tracing::error;
use uuid::Uuid;

use crate::{
    ConnectionEpochId, ConnectionEpochStatus, InboundEventStoreError, InboundEventStoreT,
    InboundMessageEnvelope, IngestMessageOutcome, IngestionCursorScope, SourceAccountRef,
    SourceEventId,
};

use super::entities::{
    secretary_accounts, secretary_connection_epochs, secretary_conversations,
    secretary_event_ingestion, secretary_ingestion_cursors, secretary_message_contents,
    secretary_source_events,
};

const ACCOUNT_ACTIVE: &str = "active";
const MEMORY_NORMAL: &str = "normal";
const PROCESSING_PENDING: &str = "pending";

pub(crate) struct MySqlInboundEventStore {
    pub(super) db: DatabaseConnection,
}

impl MySqlInboundEventStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl InboundEventStoreT for MySqlInboundEventStore {
    async fn insert_message_if_absent(
        &self,
        message: &InboundMessageEnvelope,
    ) -> Result<IngestMessageOutcome, InboundEventStoreError> {
        tracing::trace!(
            source = message.source.channel.as_str(),
            source_account_id = %message.source.account_id,
            platform_message_id = %message.source.message_id,
            conversation_kind = message.conversation.kind.as_str(),
            conversation_id = %message.conversation.id,
            connection_epoch_id = message
                .connection_epoch_id
                .as_ref()
                .map(ConnectionEpochId::as_str),
            "开始执行个人秘书消息幂等事务"
        );
        message
            .validate()
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;

        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let account_id =
            ensure_account_ref(&transaction, &message.source.account_ref(), now).await?;
        let conversation_id = ensure_conversation(&transaction, account_id, message, now).await?;
        let reply_to_event_id = resolve_reply(&transaction, account_id, message).await?;

        let proposed_event_id = Uuid::new_v4().to_string();
        let source_event = secretary_source_events::ActiveModel {
            source_event_id: Set(proposed_event_id.clone()),
            account_id: Set(account_id),
            conversation_id: Set(conversation_id),
            source_channel: Set(message.source.channel.as_str().into()),
            platform_event_id: Set(message.source.message_id.clone()),
            event_type: Set("message".into()),
            actor_platform_id: Set(message.actor.id.clone()),
            actor_kind: Set(message.actor.kind.as_str().into()),
            message_role: Set(message.role().as_str().into()),
            occurred_at_unix_secs: Set(message.occurred_at_unix_secs),
            reply_to_platform_event_id: Set(message
                .reply_to_platform_message_id()
                .map(str::to_owned)),
            reply_to_event_id: Set(reply_to_event_id.as_ref().map(|id| id.as_str().to_owned())),
            processing_status: Set(PROCESSING_PENDING.into()),
            received_at: Set(now),
            created_at: Set(now),
        };
        secretary_source_events::Entity::insert(source_event)
            .on_conflict(
                OnConflict::columns([
                    secretary_source_events::Column::AccountId,
                    secretary_source_events::Column::PlatformEventId,
                ])
                .update_column(secretary_source_events::Column::PlatformEventId)
                .to_owned(),
            )
            .exec(&transaction)
            .await
            .map_err(store_error)?;

        let stored = secretary_source_events::Entity::find()
            .filter(secretary_source_events::Column::AccountId.eq(account_id))
            .filter(
                secretary_source_events::Column::PlatformEventId
                    .eq(message.source.message_id.clone()),
            )
            .one(&transaction)
            .await
            .map_err(store_error)?
            .ok_or_else(|| {
                error!("source event vanished after idempotent insert");
                InboundEventStoreError::Unavailable
            })?;
        let source_event_id = SourceEventId::new(stored.source_event_id.clone())?;

        if stored.source_event_id != proposed_event_id {
            transaction.commit().await.map_err(store_error)?;
            tracing::trace!(
                source_event_id = %source_event_id.as_str(),
                platform_message_id = %message.source.message_id,
                "个人秘书消息事务命中重复事件"
            );
            return Ok(IngestMessageOutcome::Duplicate { source_event_id });
        }

        let segments = serde_json::to_value(&message.segments)
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        let mentioned_actor_ids =
            serde_json::to_value(message.mentioned_actor_ids().collect::<Vec<_>>())
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        let content = secretary_message_contents::ActiveModel {
            source_event_id: Set(stored.source_event_id),
            normalized_text: Set(message.normalized_text.clone()),
            segments: Set(segments),
            mentioned_actor_ids: Set(mentioned_actor_ids),
            mention_all: Set(message.mentions_all()),
            content_mode: Set(MEMORY_NORMAL.into()),
            created_at: Set(now),
        };
        secretary_message_contents::Entity::insert(content)
            .exec(&transaction)
            .await
            .map_err(store_error)?;

        if let Some(connection_epoch_id) = &message.connection_epoch_id {
            record_ingestion_continuity(
                &transaction,
                account_id,
                conversation_id,
                connection_epoch_id,
                &source_event_id,
                message,
                now,
            )
            .await?;
        }

        transaction.commit().await.map_err(store_error)?;
        tracing::debug!(
            source_event_id = %source_event_id.as_str(),
            platform_message_id = %message.source.message_id,
            connection_epoch_id = message
                .connection_epoch_id
                .as_ref()
                .map(ConnectionEpochId::as_str),
            reply_to_event_id = reply_to_event_id.as_ref().map(SourceEventId::as_str),
            "个人秘书消息幂等事务已提交"
        );
        Ok(IngestMessageOutcome::Accepted {
            source_event_id,
            reply_to_event_id,
        })
    }
}

pub(super) async fn ensure_account_ref(
    db: &sea_orm::DatabaseTransaction,
    account: &SourceAccountRef,
    now: chrono::NaiveDateTime,
) -> Result<u64, InboundEventStoreError> {
    let model = secretary_accounts::ActiveModel {
        id: NotSet,
        source_channel: Set(account.channel.as_str().into()),
        platform_account_id: Set(account.account_id.clone()),
        status: Set(ACCOUNT_ACTIVE.into()),
        created_at: Set(now),
        updated_at: Set(now),
    };
    secretary_accounts::Entity::insert(model)
        .on_conflict(
            OnConflict::columns([
                secretary_accounts::Column::SourceChannel,
                secretary_accounts::Column::PlatformAccountId,
            ])
            .update_column(secretary_accounts::Column::UpdatedAt)
            .to_owned(),
        )
        .exec(db)
        .await
        .map_err(store_error)?;
    secretary_accounts::Entity::find()
        .filter(secretary_accounts::Column::SourceChannel.eq(account.channel.as_str()))
        .filter(secretary_accounts::Column::PlatformAccountId.eq(account.account_id.clone()))
        .one(db)
        .await
        .map_err(store_error)?
        .map(|model| model.id)
        .ok_or(InboundEventStoreError::Unavailable)
}

#[allow(clippy::too_many_arguments)]
async fn record_ingestion_continuity(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    conversation_id: u64,
    connection_epoch_id: &ConnectionEpochId,
    source_event_id: &SourceEventId,
    message: &InboundMessageEnvelope,
    now: chrono::NaiveDateTime,
) -> Result<(), InboundEventStoreError> {
    let mut epoch: secretary_connection_epochs::ActiveModel =
        secretary_connection_epochs::Entity::find_by_id(connection_epoch_id.as_str().to_owned())
            .one(db)
            .await
            .map_err(store_error)?
            .filter(|epoch| {
                epoch.account_id == account_id
                    && epoch.status == ConnectionEpochStatus::Connected.as_str()
            })
            .ok_or_else(|| {
                InboundEventStoreError::InvalidData(
            "message connection epoch is missing, belongs to another account, or is not connected"
                .into(),
        )
            })?
            .into();
    epoch.last_event_at = Set(Some(now));
    epoch.last_source_event_id = Set(Some(source_event_id.as_str().to_owned()));
    epoch.updated_at = Set(now);
    epoch.update(db).await.map_err(store_error)?;

    secretary_event_ingestion::Entity::insert(secretary_event_ingestion::ActiveModel {
        source_event_id: Set(source_event_id.as_str().to_owned()),
        connection_epoch_id: Set(connection_epoch_id.as_str().to_owned()),
        observed_at: Set(now),
    })
    .exec(db)
    .await
    .map_err(store_error)?;

    upsert_cursor(
        db,
        account_id,
        None,
        IngestionCursorScope::Account.as_str(),
        "account".into(),
        connection_epoch_id,
        source_event_id,
        message,
        now,
    )
    .await?;
    upsert_cursor(
        db,
        account_id,
        Some(conversation_id),
        IngestionCursorScope::Conversation.as_str(),
        format!(
            "{}:{}",
            message.conversation.kind.as_str(),
            message.conversation.id
        ),
        connection_epoch_id,
        source_event_id,
        message,
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn upsert_cursor(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    conversation_id: Option<u64>,
    scope_kind: &str,
    scope_key: String,
    connection_epoch_id: &ConnectionEpochId,
    source_event_id: &SourceEventId,
    message: &InboundMessageEnvelope,
    now: chrono::NaiveDateTime,
) -> Result<(), InboundEventStoreError> {
    let existing = secretary_ingestion_cursors::Entity::find()
        .filter(secretary_ingestion_cursors::Column::AccountId.eq(account_id))
        .filter(secretary_ingestion_cursors::Column::ScopeKind.eq(scope_kind))
        .filter(secretary_ingestion_cursors::Column::ScopeKey.eq(scope_key.clone()))
        .one(db)
        .await
        .map_err(store_error)?;

    if let Some(existing) = existing {
        if message.occurred_at_unix_secs < existing.last_occurred_at_unix_secs {
            return Ok(());
        }
        let mut cursor: secretary_ingestion_cursors::ActiveModel = existing.into();
        cursor.conversation_id = Set(conversation_id);
        cursor.last_source_event_id = Set(source_event_id.as_str().to_owned());
        cursor.last_platform_event_id = Set(message.source.message_id.clone());
        cursor.last_occurred_at_unix_secs = Set(message.occurred_at_unix_secs);
        cursor.connection_epoch_id = Set(Some(connection_epoch_id.as_str().to_owned()));
        cursor.updated_at = Set(now);
        cursor.update(db).await.map_err(store_error)?;
    } else {
        secretary_ingestion_cursors::Entity::insert(secretary_ingestion_cursors::ActiveModel {
            id: NotSet,
            account_id: Set(account_id),
            conversation_id: Set(conversation_id),
            scope_kind: Set(scope_kind.into()),
            scope_key: Set(scope_key),
            last_source_event_id: Set(source_event_id.as_str().to_owned()),
            last_platform_event_id: Set(message.source.message_id.clone()),
            last_occurred_at_unix_secs: Set(message.occurred_at_unix_secs),
            connection_epoch_id: Set(Some(connection_epoch_id.as_str().to_owned())),
            updated_at: Set(now),
        })
        .exec(db)
        .await
        .map_err(store_error)?;
    }
    Ok(())
}

async fn ensure_conversation(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    message: &InboundMessageEnvelope,
    now: chrono::NaiveDateTime,
) -> Result<u64, InboundEventStoreError> {
    let model = secretary_conversations::ActiveModel {
        id: NotSet,
        account_id: Set(account_id),
        conversation_kind: Set(message.conversation.kind.as_str().into()),
        platform_conversation_id: Set(message.conversation.id.clone()),
        memory_mode: Set(MEMORY_NORMAL.into()),
        created_at: Set(now),
        updated_at: Set(now),
    };
    secretary_conversations::Entity::insert(model)
        .on_conflict(
            OnConflict::columns([
                secretary_conversations::Column::AccountId,
                secretary_conversations::Column::ConversationKind,
                secretary_conversations::Column::PlatformConversationId,
            ])
            .update_column(secretary_conversations::Column::UpdatedAt)
            .to_owned(),
        )
        .exec(db)
        .await
        .map_err(store_error)?;
    secretary_conversations::Entity::find()
        .filter(secretary_conversations::Column::AccountId.eq(account_id))
        .filter(
            secretary_conversations::Column::ConversationKind
                .eq(message.conversation.kind.as_str()),
        )
        .filter(
            secretary_conversations::Column::PlatformConversationId
                .eq(message.conversation.id.clone()),
        )
        .one(db)
        .await
        .map_err(store_error)?
        .map(|model| model.id)
        .ok_or(InboundEventStoreError::Unavailable)
}

async fn resolve_reply(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    message: &InboundMessageEnvelope,
) -> Result<Option<SourceEventId>, InboundEventStoreError> {
    let Some(platform_message_id) = message.reply_to_platform_message_id() else {
        return Ok(None);
    };
    secretary_source_events::Entity::find()
        .filter(secretary_source_events::Column::AccountId.eq(account_id))
        .filter(secretary_source_events::Column::PlatformEventId.eq(platform_message_id.to_owned()))
        .one(db)
        .await
        .map_err(store_error)?
        .map(|model| SourceEventId::new(model.source_event_id))
        .transpose()
}

pub(super) fn store_error(error: sea_orm::DbErr) -> InboundEventStoreError {
    error!(%error, "personal secretary inbound store operation failed");
    InboundEventStoreError::Database(error.to_string())
}
