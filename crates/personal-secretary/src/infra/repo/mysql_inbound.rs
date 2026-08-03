use async_trait::async_trait;
use chrono::Utc;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter, Set, Statement,
    TransactionTrait,
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
/// 档案/会话观察来源事件上限（与迁移 CHECK JSON_LENGTH(source_event_ids_json) <= 10 对齐）。
/// 列表满时淘汰最旧来源、保留最新建立事件，保证当前值来源始终可失效校验。
const MAX_PROFILE_SOURCE_EVENTS: usize = 10;

pub(crate) struct MySqlInboundEventStore {
    pub(super) db: DatabaseConnection,
    /// 回补运行续租秒数。仅 `record_scope_progress` 使用；实时/连续性路径不读取。
    /// 由 `build_mysql_backfill_store` 按配置注入，`build_mysql_inbound_event_store` 用默认值。
    pub(super) lease_secs: u64,
}

impl MySqlInboundEventStore {
    /// 构造实时入库/连续性仓储。不参与回补租约，使用默认值。
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db, lease_secs: 60 }
    }

    /// 构造回补状态仓储，注入配置的租约秒数。
    pub(crate) fn new_for_backfill(db: DatabaseConnection, lease_secs: u64) -> Self {
        Self { db, lease_secs }
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
            // 重复投递：仍尝试关联 pending tombstone（撤回先到、消息后到的补偿路径）。
            let source_event_id = SourceEventId::new(stored.source_event_id.clone())?;
            apply_pending_tombstone_in_txn(
                &transaction,
                account_id,
                message,
                source_event_id.as_str(),
            )
            .await?;
            transaction.commit().await.map_err(store_error)?;
            tracing::trace!(
                source_event_id = %source_event_id.as_str(),
                platform_message_id = %message.source.message_id,
                "个人秘书消息事务命中重复事件"
            );
            return Ok(IngestMessageOutcome::Duplicate { source_event_id });
        }

        let mut persisted_segments = message.segments.clone();
        for segment in &mut persisted_segments {
            if let crate::ContentSegment::Media { source_url, .. } = segment {
                *source_url = None;
            }
        }
        let segments = serde_json::to_value(&persisted_segments)
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
        transaction
            .execute_raw(sea_orm::Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::MySql,
                "INSERT IGNORE INTO secretary_artifact_derivations (source_event_id) VALUES (?)",
                [source_event_id.as_str().into()],
            ))
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

        // 父消息后到回填：本消息作为父消息，把同账号内此前因父消息尚未入库而未解析的
        // 子消息 reply_to_event_id 回填为当前事件。不跨账号、幂等。
        backfill_child_reply_edges(&transaction, account_id, &source_event_id, message).await?;

        // B3：撤回先到时的 pending tombstone，在消息 Accepted 时同事务自动关联。
        // Duplicate 路径在下方单独处理，因为 duplicate 会提前 commit。
        apply_pending_tombstone_in_txn(&transaction, account_id, message, source_event_id.as_str())
            .await?;

        // ID-005：发送者观察档案（账号级昵称/别名）与群名片/群角色（会话级观察）
        // 在同一事务内幂等 upsert。只记录显示信息，绝不构成授权；
        // never_long_term 会话或受限单事件不进入人物长期上下文。
        if let Some(profile) = &message.sender_profile {
            // 单事件 + 会话隐私门：受限观察不得进入人物长期上下文。
            if participant_observation_allowed(&transaction, conversation_id, &source_event_id)
                .await?
            {
                upsert_participant_profile_in_txn(
                    &transaction,
                    account_id,
                    &message.actor.id,
                    message.actor.kind.as_str(),
                    profile,
                    &source_event_id,
                    now,
                )
                .await?;
                // 群名片/群角色按 (account, conversation, identity_kind, actor) 会话作用域保存；
                // 私聊与控制会话没有群属性，不产生观察行。
                if message.conversation.kind == crate::ConversationKind::Group {
                    upsert_participant_conversation_observation_in_txn(
                        &transaction,
                        account_id,
                        conversation_id,
                        message.actor.kind.as_str(),
                        &message.actor.id,
                        profile,
                        &source_event_id,
                        now,
                    )
                    .await?;
                }
            } else {
                tracing::trace!(
                    source_event_id = %source_event_id.as_str(),
                    "受限观察不进入人物上下文"
                );
            }
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
        policy_epoch: Set(0),
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

/// 父消息后到回填：当一条消息被接受入库时，把它作为父消息，把同账号内所有引用其
/// 平台消息 ID、但当时因父消息尚未入库而 `reply_to_event_id` 为空的子消息回填。
///
/// 不变规则：
/// - 只回填同一账号主体（`account_id`），绝不跨账号绑定 Reply；
/// - 幂等：再次执行只更新仍为空的行，已回填的行保持不变；
/// - 在父消息插入事务内完成，保证原子可见性。
async fn backfill_child_reply_edges(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    parent_event_id: &SourceEventId,
    parent: &InboundMessageEnvelope,
) -> Result<(), InboundEventStoreError> {
    // 把同账号内 reply_to_platform_event_id = 父平台消息 ID 且 reply_to_event_id 为空
    // 的子消息批量回填。不依赖子消息是否已入库，缺失时自然不影响。
    secretary_source_events::Entity::update_many()
        .col_expr(
            secretary_source_events::Column::ReplyToEventId,
            Expr::value(parent_event_id.as_str()),
        )
        .filter(secretary_source_events::Column::AccountId.eq(account_id))
        .filter(
            secretary_source_events::Column::ReplyToPlatformEventId
                .eq(parent.source.message_id.clone()),
        )
        .filter(secretary_source_events::Column::ReplyToEventId.is_null())
        .exec(db)
        .await
        .map_err(store_error)?;
    Ok(())
}

pub(super) fn store_error(error: sea_orm::DbErr) -> InboundEventStoreError {
    error!(%error, "personal secretary inbound store operation failed");
    InboundEventStoreError::Database(error.to_string())
}

/// B3：在消息入库事务内把匹配的 pending tombstone 转为 applied，并传播 Artifact 失效。
async fn apply_pending_tombstone_in_txn(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    message: &InboundMessageEnvelope,
    source_event_id: &str,
) -> Result<(), InboundEventStoreError> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    let correlation_key = format!(
        "{}:{}:{}:{}:{}",
        message.source.channel.as_str(),
        message.source.account_id,
        message.conversation.kind.as_str(),
        message.conversation.id,
        message.source.message_id
    );

    let update = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"UPDATE secretary_message_tombstones
               SET source_event_id = ?, status = 'applied',
                   invalidation_reason = 'original message arrived after recall',
                   invalidated_at_unix_secs = UNIX_TIMESTAMP()
               WHERE account_id = ? AND correlation_key = ? AND status = 'pending'"#,
            [
                source_event_id.into(),
                account_id.into(),
                correlation_key.into(),
            ],
        ))
        .await
        .map_err(store_error)?;

    if update.rows_affected() > 0 {
        // 同步传播 Artifact 失效；失败必须回滚整个消息事务，禁止 tombstone=applied 但 artifact 仍 available。
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"UPDATE secretary_artifacts
               SET availability = 'recalled'
               WHERE source_event_id = ? AND availability = 'available'"#,
            [source_event_id.into()],
        ))
        .await
        .map_err(store_error)?;
        tracing::debug!(
            source_event_id,
            platform_message_id = %message.source.message_id,
            "消息入库事务内已应用 pending 撤回 tombstone 并传播 Artifact 失效"
        );
    }
    Ok(())
}

/// 单事件 + 会话隐私门（fail-closed）：正文投影缺失、事件/会话为 never_long_term 或
/// 事件已被撤回时，该观察不得进入人物长期上下文；envelope_only 仍保留信封级身份。
#[derive(Debug, FromQueryResult)]
struct EventPrivacyRow {
    content_mode: Option<String>,
    tombstone_status: Option<String>,
}

async fn participant_observation_allowed(
    db: &sea_orm::DatabaseTransaction,
    conversation_id: u64,
    source_event_id: &SourceEventId,
) -> Result<bool, InboundEventStoreError> {
    let event_privacy = EventPrivacyRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT m.content_mode, t.status AS tombstone_status
           FROM secretary_message_contents m
           LEFT JOIN secretary_message_tombstones t
             ON t.source_event_id = m.source_event_id AND t.status = 'applied'
           WHERE m.source_event_id = ?"#,
        [source_event_id.as_str().into()],
    ))
    .one(db)
    .await
    .map_err(store_error)?;
    let Some(event_privacy) = event_privacy else {
        tracing::trace!(
            source_event_id = %source_event_id.as_str(),
            "事件正文投影缺失，跳过参与者观察"
        );
        return Ok(false);
    };
    if event_privacy.tombstone_status.is_some() {
        tracing::trace!(
            source_event_id = %source_event_id.as_str(),
            "事件已被撤回，跳过参与者观察"
        );
        return Ok(false);
    }
    if event_privacy.content_mode.as_deref() == Some("never_long_term") {
        tracing::trace!(
            source_event_id = %source_event_id.as_str(),
            "事件为 never_long_term，跳过参与者观察"
        );
        return Ok(false);
    }
    let memory_mode: String = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT memory_mode FROM secretary_conversations WHERE id = ?",
            [conversation_id.into()],
        ))
        .await
        .map_err(store_error)?
        .ok_or_else(|| {
            InboundEventStoreError::InvalidData(
                "conversation vanished during profile upsert".into(),
            )
        })?
        .try_get("", "memory_mode")
        .map_err(|error| {
            InboundEventStoreError::InvalidData(format!(
                "conversation memory_mode decode failed: {error}"
            ))
        })?;
    if memory_mode == "never_long_term" {
        tracing::trace!(
            source_event_id = %source_event_id.as_str(),
            "会话为 never_long_term，跳过参与者观察"
        );
        return Ok(false);
    }
    Ok(true)
}

/// ID-005：发送者观察档案的幂等 upsert（同一入站事务内执行）。
///
/// 不变量：
/// - 同一 (account_id, actor_platform_id, current=1) 至多一行（仅当前版本参与唯一
///   约束，历史版本可无限累积）；显示信息变化时旧行 `current=0` 保留审计，
///   旧显示名进入有界 aliases（≤10 条、每项 ≤200 字符、去重）；
/// - 来源事件 ID 追加进 `source_event_ids_json`（≤10 个，去重）；
/// - 昵称只是显示信息，绝不参与授权（群名片/群角色见会话观察 upsert）；
/// - 调用方已通过 `participant_observation_allowed` 完成单事件/会话隐私门。
#[allow(clippy::too_many_arguments)]
async fn upsert_participant_profile_in_txn(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    actor_platform_id: &str,
    platform_identity_kind: &str,
    profile: &crate::ObservedSenderProfile,
    source_event_id: &SourceEventId,
    now: chrono::NaiveDateTime,
) -> Result<(), InboundEventStoreError> {
    let display_name = profile.nickname.chars().take(200).collect::<String>();

    #[derive(Debug, FromQueryResult)]
    struct CurrentProfileRow {
        profile_id: u64,
        display_name: String,
        aliases_json: String,
        source_event_ids_json: String,
        established_by_event_id: Option<String>,
    }

    let current = CurrentProfileRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT profile_id, display_name,
                  CAST(aliases_json AS CHAR) AS aliases_json,
                  CAST(source_event_ids_json AS CHAR) AS source_event_ids_json,
                  established_by_event_id
           FROM secretary_participant_profiles
           WHERE account_id = ? AND platform_identity_kind = ?
             AND actor_platform_id = ? AND current = 1
           LIMIT 1"#,
        [
            account_id.into(),
            platform_identity_kind.into(),
            actor_platform_id.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(store_error)?;

    // 有界追加来源事件（≤10 个，去重）。列表已满时必须淘汰最旧来源、保留最新
    // 建立事件：当前值（显示名/名片/角色）仍由最新事件建立，若把它截断丢弃，
    // 该事件被撤回/删除后读取侧将无法发现当前值失效（P0 反例）。
    let append_source =
        |current_json: &str| -> Result<Vec<serde_json::Value>, InboundEventStoreError> {
            let mut ids: Vec<String> = if current_json.trim().is_empty() || current_json == "null" {
                Vec::new()
            } else {
                serde_json::from_str(current_json).map_err(|error| {
                    InboundEventStoreError::InvalidData(format!(
                        "participant profile source_event_ids_json decode failed: {error}"
                    ))
                })?
            };
            if !ids.iter().any(|id| id == source_event_id.as_str()) {
                if ids.len() >= MAX_PROFILE_SOURCE_EVENTS {
                    ids.remove(0);
                }
                ids.push(source_event_id.as_str().to_owned());
            }
            Ok(ids
                .into_iter()
                .map(serde_json::Value::String)
                .collect::<Vec<_>>())
        };

    let Some(current) = current else {
        // 首次观察：直接插入 current 档案。
        let sources = append_source("")?;
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"INSERT INTO secretary_participant_profiles
               (account_id, platform_identity_kind, actor_platform_id, display_name,
                aliases_json, trust, confirmed, invalidated, source_event_ids_json,
                established_by_event_id, current, first_seen_at, updated_at)
               VALUES (?, ?, ?, ?, ?, 'observed', 0, 0, ?, ?, 1, ?, ?)"#,
            [
                account_id.into(),
                platform_identity_kind.into(),
                actor_platform_id.into(),
                display_name.clone().into(),
                serde_json::json!(Vec::<serde_json::Value>::new())
                    .to_string()
                    .into(),
                serde_json::json!(sources).to_string().into(),
                source_event_id.as_str().into(),
                now.into(),
                now.into(),
            ],
        ))
        .await
        .map_err(store_error)?;
        tracing::trace!(
            source_event_id = %source_event_id.as_str(),
            "已写入参与者档案首条观察"
        );
        return Ok(());
    };

    if current.display_name == display_name {
        // 观察无变化：只追加来源事件。
        let sources = append_source(&current.source_event_ids_json)?;
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_participant_profiles SET source_event_ids_json = ? WHERE profile_id = ?",
            [
                serde_json::json!(sources).to_string().into(),
                current.profile_id.into(),
            ],
        ))
        .await
        .map_err(store_error)?;
        return Ok(());
    }

    // 显示信息变化：旧行 current=0 保留审计，旧显示名进入有界 aliases（去重、≤10、≤200 字符）。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_participant_profiles SET current = 0 WHERE profile_id = ?",
        [current.profile_id.into()],
    ))
    .await
    .map_err(store_error)?;

    let mut aliases: Vec<serde_json::Value> =
        if current.aliases_json.trim().is_empty() || current.aliases_json == "null" {
            Vec::new()
        } else {
            serde_json::from_str(&current.aliases_json).map_err(|error| {
                InboundEventStoreError::InvalidData(format!(
                    "participant profile aliases_json decode failed: {error}"
                ))
            })?
        };
    if !current.display_name.is_empty()
        && !aliases.iter().any(|alias| {
            alias.get("alias").and_then(serde_json::Value::as_str)
                == Some(current.display_name.as_str())
        })
    {
        // alias 的来源是建立该旧显示名的来源事件（established_by_event_id 精确
        // 跟踪），绝不是触发本次变化的新事件；同名消息追加的来源不影响该精度。
        let alias_source = current
            .established_by_event_id
            .clone()
            .unwrap_or_else(|| source_event_id.as_str().to_owned());
        aliases.push(serde_json::json!({
            "alias": current.display_name.chars().take(200).collect::<String>(),
            "source_event_id": alias_source,
        }));
    }
    aliases.truncate(10);
    let sources = append_source(&current.source_event_ids_json)?;

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"INSERT INTO secretary_participant_profiles
           (account_id, platform_identity_kind, actor_platform_id, display_name,
            aliases_json, trust, confirmed, invalidated, source_event_ids_json,
            established_by_event_id, current, first_seen_at, updated_at)
           VALUES (?, ?, ?, ?, ?, 'observed', 0, 0, ?, ?, 1, ?, ?)"#,
        [
            account_id.into(),
            platform_identity_kind.into(),
            actor_platform_id.into(),
            display_name.into(),
            serde_json::json!(aliases).to_string().into(),
            serde_json::json!(sources).to_string().into(),
            source_event_id.as_str().into(),
            now.into(),
            now.into(),
        ],
    ))
    .await
    .map_err(store_error)?;
    tracing::trace!(
        source_event_id = %source_event_id.as_str(),
        "参与者档案显示信息已更新，旧显示名进入有界 aliases"
    );
    Ok(())
}

/// 会话（群）作用域观察：群名片/群角色按 (account, conversation, actor) 保存。
/// 同一 Actor 在不同群的名片/角色互不覆盖；只显示不授权；
/// 单事件受限（投影缺失 / never_long_term / 已撤回）同样跳过。
#[allow(clippy::too_many_arguments)]
async fn upsert_participant_conversation_observation_in_txn(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    conversation_id: u64,
    platform_identity_kind: &str,
    actor_platform_id: &str,
    profile: &crate::ObservedSenderProfile,
    source_event_id: &SourceEventId,
    now: chrono::NaiveDateTime,
) -> Result<(), InboundEventStoreError> {
    let group_card = profile
        .group_card
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect::<String>();
    // 协议群角色必须归一化：缺失或未知值一律 unknown，否则 CHECK 约束会把
    // 未知角色字符串打成整条消息入库失败。
    let group_role = crate::GroupRole::parse_protocol(profile.group_role.as_deref())
        .as_str()
        .to_owned();

    #[derive(Debug, FromQueryResult)]
    struct ObservationRow {
        observation_id: u64,
        group_card: Option<String>,
        group_role: String,
        established_by_event_id: Option<String>,
        source_event_ids_json: String,
    }
    let current = ObservationRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT observation_id, group_card, group_role, established_by_event_id,
                  CAST(source_event_ids_json AS CHAR) AS source_event_ids_json
           FROM secretary_participant_conversation_observations
           WHERE account_id = ? AND conversation_id = ?
             AND platform_identity_kind = ? AND actor_platform_id = ?"#,
        [
            account_id.into(),
            conversation_id.into(),
            platform_identity_kind.into(),
            actor_platform_id.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(store_error)?;

    let Some(current) = current else {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"INSERT INTO secretary_participant_conversation_observations
               (account_id, conversation_id, platform_identity_kind, actor_platform_id,
                group_card, group_role, established_by_event_id, source_event_ids_json,
                first_seen_at, updated_at)
               VALUES (?, ?, ?, ?, NULLIF(?, ''), ?, ?, ?, ?, ?)"#,
            [
                account_id.into(),
                conversation_id.into(),
                platform_identity_kind.into(),
                actor_platform_id.into(),
                group_card.clone().into(),
                group_role.into(),
                source_event_id.as_str().into(),
                serde_json::json!(vec![source_event_id.as_str()])
                    .to_string()
                    .into(),
                now.into(),
                now.into(),
            ],
        ))
        .await
        .map_err(store_error)?;
        tracing::trace!(
            source_event_id = %source_event_id.as_str(),
            "已写入会话作用域群名片/群角色观察"
        );
        return Ok(());
    };

    // 有界追加来源（≤10，去重）；列表满时淘汰最旧来源、保留最新建立事件
    // （当前名片/角色由最新事件建立，不得在截断中丢失）。
    let mut sources: Vec<String> = if current.source_event_ids_json.trim().is_empty()
        || current.source_event_ids_json == "null"
    {
        Vec::new()
    } else {
        serde_json::from_str(&current.source_event_ids_json).map_err(|error| {
            InboundEventStoreError::InvalidData(format!(
                "conversation observation source_event_ids_json decode failed: {error}"
            ))
        })?
    };
    if !sources.iter().any(|id| id == source_event_id.as_str()) {
        if sources.len() >= MAX_PROFILE_SOURCE_EVENTS {
            sources.remove(0);
        }
        sources.push(source_event_id.as_str().to_owned());
    }
    // 名片或角色真正变化时，当前值由本次事件建立（独立失效校验用）；
    // 仅追加来源、值未变化时保留原建立事件。
    let established = if current.group_card.as_deref() != Some(group_card.as_str())
        || current.group_role != group_role
    {
        source_event_id.as_str().to_owned()
    } else {
        current.established_by_event_id.unwrap_or_default()
    };
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"UPDATE secretary_participant_conversation_observations
           SET group_card = NULLIF(?, ''), group_role = ?,
               established_by_event_id = NULLIF(?, ''),
               source_event_ids_json = ?, updated_at = ?
           WHERE observation_id = ?"#,
        [
            group_card.into(),
            group_role.into(),
            established.into(),
            serde_json::json!(sources).to_string().into(),
            now.into(),
            current.observation_id.into(),
        ],
    ))
    .await
    .map_err(store_error)?;
    tracing::trace!(
        source_event_id = %source_event_id.as_str(),
        "会话作用域群名片/群角色观察已更新"
    );
    Ok(())
}
