use async_trait::async_trait;
use chrono::Utc;
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
        let mut results = self
            .insert_messages_if_absent(std::slice::from_ref(message))
            .await?;
        Ok(results
            .pop()
            .expect("batch insert must return one result for singleton"))
    }

    async fn insert_messages_if_absent(
        &self,
        messages: &[InboundMessageEnvelope],
    ) -> Result<Vec<IngestMessageOutcome>, InboundEventStoreError> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        // 前置校验：任意消息不满足结构不变量时立即返回 InvalidData，
        // 让 Worker 二分隔离 poison 消息。
        for message in messages {
            message
                .validate()
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        }

        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let mut outcomes = Vec::with_capacity(messages.len());

        for message in messages {
            match process_message_in_transaction(&transaction, message, now).await {
                Ok(outcome) => outcomes.push(outcome),
                Err(error) => {
                    let _ = transaction.rollback().await;
                    return Err(error);
                }
            }
        }

        transaction.commit().await.map_err(store_error)?;

        // EVT-007 并发交错自愈：父事件可能在子事件事务期间才提交，子事务内的
        // resolve_reply 看不到尚未提交的父事件。提交后对带 Reply 段的消息再次
        // 幂等解析。
        //
        // 契约：主批事务一旦提交，本调用必须返回成功——消息已持久化，绝不把
        // "已提交" 伪报为失败。自愈失败只记类型化错误日志（不记录平台消息 ID 等
        // 稳定外部标识）：待解析状态已随 SourceEvent 持久化（unresolved），
        // 父事件后续经 Duplicate 重放/回补到达时，事务内回填路径仍会完成解析；
        // 若父事件永不重放，后台 Reply 修复 Worker 按退避周期持续重试，形成
        // 持久化可恢复闭环。
        let mut self_heal_failures = 0u32;
        for message in messages {
            if message.reply_to_platform_message_id().is_some()
                && let Err(error) = self.resolve_pending_reply_after_commit(message).await
            {
                self_heal_failures = self_heal_failures.saturating_add(1);
                let _ = &error;
            }
        }
        if self_heal_failures > 0 {
            tracing::error!(
                error_code = "delayed_reply_self_heal_failed",
                stage = "post_commit",
                failed_count = self_heal_failures,
                "消息批次已提交，但提交后 Reply 自愈失败；保持 unresolved，等待父事件重放或后台修复 Worker 重试"
            );
        }

        for outcome in &outcomes {
            tracing::debug!(
                source_event_id = %outcome.source_event_id().as_str(),
                platform_message_id = match outcome {
                    IngestMessageOutcome::Accepted { .. } => "batch-accepted",
                    IngestMessageOutcome::Duplicate { .. } => "batch-duplicate",
                },
                "批量消息幂等事务已提交"
            );
        }
        Ok(outcomes)
    }
}

impl MySqlInboundEventStore {
    /// EVT-007 提交后自愈：子事件事务提交后，父事件可能恰在此窗口提交（并发交错），
    /// 子事务内的首次查找因此落空。提交后重查同账号、同通道、同会话的父事件；
    /// 存在则把本消息及同会话内所有匹配的 pending 子事件解析为正式关系。
    ///
    /// 幂等：父事件不存在或无 pending 子事件时无副作用；失败整体回滚且可重试，
    /// 消息重放为 Duplicate 后仍会再次进入本入口。绝不在提交前触发任何派生观察。
    async fn resolve_pending_reply_after_commit(
        &self,
        child: &InboundMessageEnvelope,
    ) -> Result<(), InboundEventStoreError> {
        let parent_platform_id = child
            .reply_to_platform_message_id()
            .expect("caller only invokes for messages with a reply segment");
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let account_id = ensure_account_ref(&transaction, &child.source.account_ref(), now).await?;
        let conversation_id = ensure_conversation(&transaction, account_id, child, now).await?;
        let Some(parent_event_id) =
            resolve_reply(&transaction, account_id, conversation_id, child).await?
        else {
            // 父事件仍不可见：保持 pending，等待父事件未来到达（实时/回补/重放）。
            transaction.commit().await.map_err(store_error)?;
            return Ok(());
        };
        resolve_pending_replies_in_txn(
            &transaction,
            account_id,
            conversation_id,
            child.source.channel.as_str(),
            parent_platform_id,
            &parent_event_id,
        )
        .await?;
        transaction.commit().await.map_err(store_error)?;
        Ok(())
    }
}

/// 在已有事务内处理单条消息，不提交、不回滚。
async fn process_message_in_transaction(
    transaction: &sea_orm::DatabaseTransaction,
    message: &InboundMessageEnvelope,
    now: chrono::NaiveDateTime,
) -> Result<IngestMessageOutcome, InboundEventStoreError> {
    let account_id = ensure_account_ref(transaction, &message.source.account_ref(), now).await?;
    let conversation_id = ensure_conversation(transaction, account_id, message, now).await?;
    let reply_to_event_id =
        resolve_reply(transaction, account_id, conversation_id, message).await?;

    let proposed_event_id = Uuid::new_v4().to_string();
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"INSERT INTO secretary_source_events
               (source_event_id, account_id, conversation_id, source_channel,
                platform_event_id, event_type, actor_platform_id, actor_kind,
                message_role, occurred_at_unix_secs, reply_to_platform_event_id,
                reply_to_event_id, processing_status, received_at, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON DUPLICATE KEY UPDATE platform_event_id = VALUES(platform_event_id)"#,
            [
                proposed_event_id.clone().into(),
                account_id.into(),
                conversation_id.into(),
                message.source.channel.as_str().into(),
                message.source.message_id.clone().into(),
                "message".into(),
                message.actor.id.clone().into(),
                message.actor.kind.as_str().into(),
                message.role().as_str().into(),
                message.occurred_at_unix_secs.into(),
                message
                    .reply_to_platform_message_id()
                    .map(str::to_owned)
                    .into(),
                reply_to_event_id
                    .as_ref()
                    .map(|id| id.as_str().to_owned())
                    .into(),
                PROCESSING_PENDING.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await
        .map_err(store_error)?;

    let stored = secretary_source_events::Entity::find()
        .filter(secretary_source_events::Column::AccountId.eq(account_id))
        .filter(
            secretary_source_events::Column::PlatformEventId.eq(message.source.message_id.clone()),
        )
        .one(transaction)
        .await
        .map_err(store_error)?
        .ok_or_else(|| {
            error!("source event vanished after idempotent insert");
            InboundEventStoreError::Unavailable
        })?;
    let source_event_id = SourceEventId::new(stored.source_event_id.clone())?;

    if stored.source_event_id != proposed_event_id {
        // 重复投递：在批量事务内仍关联 pending tombstone，且父事件重放必须
        // 同样尝试解析此前未完成的待解析 Reply 关系（EVT-007）。
        apply_pending_tombstone_in_txn(transaction, account_id, message, source_event_id.as_str())
            .await?;
        resolve_pending_replies_in_txn(
            transaction,
            account_id,
            conversation_id,
            message.source.channel.as_str(),
            &message.source.message_id,
            &source_event_id,
        )
        .await?;
        // Duplicate 且仍 unresolved：幂等入队，INSERT IGNORE 不覆盖已有退避。
        if stored.reply_to_event_id.is_none() && stored.reply_to_platform_event_id.is_some() {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "INSERT IGNORE INTO secretary_reply_reconcile_claims (source_event_id) VALUES (?)",
                    [source_event_id.as_str().into()],
                ))
                .await
                .map_err(store_error)?;
        }
        return Ok(IngestMessageOutcome::Duplicate { source_event_id });
    }

    // Accepted 路径：写入内容、回填回复边、关联 tombstone、更新观察档案。
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
        .exec(transaction)
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
            transaction,
            account_id,
            conversation_id,
            connection_epoch_id,
            &source_event_id,
            message,
            now,
        )
        .await?;
    }

    // 父事件入库：同账号、同会话内引用其平台消息 ID 的 pending 子事件在本事务内
    // 解析为正式 Reply 关系（EVT-007 父后到路径）。
    resolve_pending_replies_in_txn(
        transaction,
        account_id,
        conversation_id,
        message.source.channel.as_str(),
        &message.source.message_id,
        &source_event_id,
    )
    .await?;

    apply_pending_tombstone_in_txn(transaction, account_id, message, source_event_id.as_str())
        .await?;

    if let Some(profile) = &message.sender_profile
        && participant_observation_allowed(transaction, conversation_id, &source_event_id).await?
    {
        upsert_participant_profile_in_txn(
            transaction,
            account_id,
            &message.actor.id,
            message.actor.kind.as_str(),
            profile,
            &source_event_id,
            now,
        )
        .await?;
        if message.conversation.kind == crate::ConversationKind::Group {
            upsert_participant_conversation_observation_in_txn(
                transaction,
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
    }

    // 候选队列（Codex 第四轮复核 #5）：unresolved Reply 子事件同一事务入队，
    // 已即时解析的子事件不入队。Duplicate 重放时 INSERT IGNORE 不覆盖已有退避。
    if reply_to_event_id.is_none() && message.reply_to_platform_message_id().is_some() {
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT IGNORE INTO secretary_reply_reconcile_claims (source_event_id) VALUES (?)",
                [source_event_id.as_str().into()],
            ))
            .await
            .map_err(store_error)?;
    }

    Ok(IngestMessageOutcome::Accepted {
        source_event_id,
        reply_to_event_id,
    })
}

pub(super) async fn ensure_account_ref(
    db: &sea_orm::DatabaseTransaction,
    account: &SourceAccountRef,
    now: chrono::NaiveDateTime,
) -> Result<u64, InboundEventStoreError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"INSERT INTO secretary_accounts
           (source_channel, platform_account_id, status, policy_epoch, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?)
           ON DUPLICATE KEY UPDATE updated_at = VALUES(updated_at)"#,
        [
            account.channel.as_str().into(),
            account.account_id.clone().into(),
            ACCOUNT_ACTIVE.into(),
            0_i32.into(),
            now.into(),
            now.into(),
        ],
    ))
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
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"INSERT INTO secretary_conversations
           (account_id, conversation_kind, platform_conversation_id, memory_mode, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?)
           ON DUPLICATE KEY UPDATE updated_at = VALUES(updated_at)"#,
        [
            account_id.into(),
            message.conversation.kind.as_str().into(),
            message.conversation.id.clone().into(),
            MEMORY_NORMAL.into(),
            now.into(),
            now.into(),
        ],
    ))
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

pub(super) async fn resolve_reply(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    conversation_id: u64,
    message: &InboundMessageEnvelope,
) -> Result<Option<SourceEventId>, InboundEventStoreError> {
    let Some(platform_message_id) = message.reply_to_platform_message_id() else {
        return Ok(None);
    };
    resolve_reply_by_refs(
        db,
        account_id,
        conversation_id,
        message.source.channel.as_str(),
        platform_message_id,
    )
    .await
}

/// 同作用域父事件查找（envelope 版与后台修复版共用的核心）。
/// EVT-007 会话边界：父事件必须与子消息同账号、同来源通道、同会话。
/// 相同平台消息 ID 出现在不同账号/不同群/私聊时必须 fail-closed（不解析），
/// 让子事件保持 pending，等待真正同会话的父事件到达。
pub(super) async fn resolve_reply_by_refs(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    conversation_id: u64,
    source_channel: &str,
    platform_message_id: &str,
) -> Result<Option<SourceEventId>, InboundEventStoreError> {
    secretary_source_events::Entity::find()
        .filter(secretary_source_events::Column::AccountId.eq(account_id))
        .filter(secretary_source_events::Column::ConversationId.eq(conversation_id))
        .filter(secretary_source_events::Column::SourceChannel.eq(source_channel))
        .filter(secretary_source_events::Column::PlatformEventId.eq(platform_message_id.to_owned()))
        .one(db)
        .await
        .map_err(store_error)?
        .map(|model| SourceEventId::new(model.source_event_id))
        .transpose()
}

/// 在已有事务内把匹配的 pending Reply 子事件解析为正式关系，并失效它们此前
/// 错误的线程投影，让确定性线程投影最终重新处理（EVT-007：父关系后到不得永久
/// 保留错误线程）。
///
/// 调用时机：
/// - 父事件实时/回补入库（Accepted）时；
/// - 父事件 Duplicate 重放时（重放必须同样修复此前未完成的待解析关系）；
/// - 子事件已提交而父事件恰在交错窗口提交时（提交后自愈入口）。
///
/// 不变规则：
/// - 只解析同 `account_id`、同 `source_channel`、同 `conversation_id` 的子事件；
///   跨账号/跨会话同名父平台消息 ID 必须 fail-closed（不解析、不失效投影）；
/// - 幂等：`reply_to_event_id IS NULL` 条件保证重复调用只推进仍未解析的行；
///   UPDATE 自身加行锁，并发父/子 Worker 不会重复回填同一子事件；
/// - 正式关系写入与线程投影失效在同一事务提交，失败整体回滚；
/// - 投影失效只针对本次解析（`reply_to_event_id` 已等于父）的子事件，已解析的
///   其他子事件不受影响；
/// - 投影租约（claims）同步撤销：已领取未提交的旧计划在 commit 时因租约检查
///   （count != assignments.len）判为 LeaseLost，不能把子事件写回旧线程；
/// - 只删除确定性投影生成的边（reply/same_conversation_window/
///   same_actor_within_conversation_window），explicit_project_id/file_version
///   等非本次修复产生的证据边必须保留；
/// - 子事件离开后变空的旧线程标记为 closed（authority=system_recovery）并清除
///   其语义批处理状态，避免幽灵 open 线程永久计入 Owner 状态统计。
pub(super) async fn resolve_pending_replies_in_txn(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    conversation_id: u64,
    parent_channel: &str,
    parent_platform_event_id: &str,
    parent_event_id: &SourceEventId,
) -> Result<usize, InboundEventStoreError> {
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"UPDATE secretary_source_events
               SET reply_to_event_id = ?
               WHERE account_id = ?
                 AND conversation_id = ?
                 AND source_channel = ?
                 AND reply_to_platform_event_id = ?
                 AND reply_to_event_id IS NULL"#,
            [
                parent_event_id.as_str().into(),
                account_id.into(),
                conversation_id.into(),
                parent_channel.into(),
                parent_platform_event_id.into(),
            ],
        ))
        .await
        .map_err(store_error)?;
    let resolved = updated.rows_affected();
    if resolved == 0 {
        return Ok(0);
    }
    // 后台修复 Worker 的退避簿行随解析一并清理（任何解析路径都不残留已解析候选；
    // reconcile 内部调用本函数时重复清理幂等无害）。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"DELETE r FROM secretary_reply_reconcile_claims r
           JOIN secretary_source_events s
             ON s.source_event_id = r.source_event_id
           WHERE s.account_id = ?
             AND s.conversation_id = ?
             AND s.source_channel = ?
             AND s.reply_to_platform_event_id = ?
             AND s.reply_to_event_id IS NOT NULL"#,
        [
            account_id.into(),
            conversation_id.into(),
            parent_channel.into(),
            parent_platform_event_id.into(),
        ],
    ))
    .await
    .map_err(store_error)?;
    // 先收集受影响子事件的旧线程（成员行删除前），随后逐线程处理空线程。
    let old_threads = ThreadIdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT te.thread_id AS value
           FROM secretary_thread_events te
           JOIN secretary_source_events child
             ON child.source_event_id = te.source_event_id
           WHERE child.account_id = ?
             AND child.conversation_id = ?
             AND child.source_channel = ?
             AND child.reply_to_platform_event_id = ?
             AND child.reply_to_event_id = ?"#,
        [
            account_id.into(),
            conversation_id.into(),
            parent_channel.into(),
            parent_platform_event_id.into(),
            parent_event_id.as_str().into(),
        ],
    ))
    .all(db)
    .await
    .map_err(store_error)?;

    // 失效旧线程成员。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"DELETE te FROM secretary_thread_events te
           JOIN secretary_source_events child
             ON child.source_event_id = te.source_event_id
           WHERE child.account_id = ?
             AND child.conversation_id = ?
             AND child.source_channel = ?
             AND child.reply_to_platform_event_id = ?
             AND child.reply_to_event_id = ?"#,
        [
            account_id.into(),
            conversation_id.into(),
            parent_channel.into(),
            parent_platform_event_id.into(),
            parent_event_id.as_str().into(),
        ],
    ))
    .await
    .map_err(store_error)?;
    // 撤销已领取未提交的投影租约：旧计划的 commit 租约检查随后判 LeaseLost。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"DELETE c FROM secretary_thread_projection_claims c
           JOIN secretary_source_events child
             ON child.source_event_id = c.source_event_id
           WHERE child.account_id = ?
             AND child.conversation_id = ?
             AND child.source_channel = ?
             AND child.reply_to_platform_event_id = ?
             AND child.reply_to_event_id = ?"#,
        [
            account_id.into(),
            conversation_id.into(),
            parent_channel.into(),
            parent_platform_event_id.into(),
            parent_event_id.as_str().into(),
        ],
    ))
    .await
    .map_err(store_error)?;
    // 删除受影响子事件的所有入边和出边。
    // 事件迁入父线程后，关系的任一端点不再属于原线程，当前关系模型无
    // historical/active 语义或跨线程 link 模型，两条边都必须清除
    // （Codex 第四轮复核 #3）。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"DELETE r FROM secretary_thread_relations r
           JOIN secretary_source_events child
             ON child.source_event_id = r.from_event_id
             OR child.source_event_id = r.to_event_id
           WHERE child.account_id = ?
             AND child.conversation_id = ?
             AND child.source_channel = ?
             AND child.reply_to_platform_event_id = ?
             AND child.reply_to_event_id = ?"#,
        [
            account_id.into(),
            conversation_id.into(),
            parent_channel.into(),
            parent_platform_event_id.into(),
            parent_event_id.as_str().into(),
        ],
    ))
    .await
    .map_err(store_error)?;

    // 处理变空的旧线程：无剩余成员时标记 closed 并清除语义批处理状态，
    // 避免幽灵 open 线程永久计入 Owner 状态统计（retriever 按 open/reopened 计数）。
    let now = Utc::now().naive_utc();
    for row in old_threads {
        close_empty_thread_in_txn(db, &row.value, now).await?;
    }
    tracing::debug!(
        resolved_count = resolved,
        "已解析同会话 pending Reply 并失效旧线程投影"
    );
    Ok(resolved as usize)
}

/// 旧线程成员行收集（value = thread_id）。
#[derive(Debug, FromQueryResult)]
struct ThreadIdRow {
    value: String,
}

/// 子事件离开后，若旧线程已无任何成员，则把线程标记为 `closed`
/// （authority=system_recovery）并清除其语义批处理状态与租约。
///
/// 不删除线程行：`secretary_event_threads` 被 thread_relations（含非确定性边）与
/// owner_controls（ON DELETE RESTRICT）等引用，删除会破坏既有证据与 Owner 操作记录。
/// closed 后 retriever 的 open/reopened 统计不再计入，线程生命周期保留在
/// status_history 中可审计。
async fn close_empty_thread_in_txn(
    db: &sea_orm::DatabaseTransaction,
    thread_id: &str,
    now: chrono::NaiveDateTime,
) -> Result<(), InboundEventStoreError> {
    // 并发安全（Codex 复核 P1-2/P1-3）：
    // - 先对线程行加共享锁并读当前状态：投影事务（commit_projection 对线程行加
    //   FOR UPDATE 锁）与语义事务（状态迁移 UPDATE）都与之互斥，读取不漂移；
    // - 关闭判定是原子条件 UPDATE：`status = 锁内读到的状态 AND NOT EXISTS 成员`，
    //   在同一把行锁下复验成员与状态，检查影响行数——并发投影先插入成员时本 UPDATE
    //   影响 0 行直接返回；并发语义事务先把状态改为终态时同样 0 行，绝不写虚假历史；
    // - 锁内复核通过后，语义派生（claims/decisions/open questions/expectations）在
    //   同一事务撤销，in-flight 语义 commit 要么因租约行被删除判 LeaseLost，要么先
    //   提交后由本事务撤销，两种顺序都收敛为"旧线程无活跃语义派生"。
    // - 终态线程分离处理（Codex 第三轮复核 P1-2）：已处 resolved/closed 的线程不再
    //   写关闭历史，但仍必须在锁内复验为空并撤销已提交的语义派生——事件已迁走，
    //   残留的 claims/decisions/questions/expectations 与租约必须清除。
    let thread_row = ThreadStatusRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT status FROM secretary_event_threads WHERE thread_id = ? FOR SHARE",
        [thread_id.into()],
    ))
    .one(db)
    .await
    .map_err(store_error)?;
    let Some(thread_row) = thread_row else {
        return Ok(());
    };
    let is_terminal = matches!(thread_row.status.as_str(), "resolved" | "closed");

    if is_terminal {
        // 终态线程：不写关闭历史，但在锁内确认无成员后撤销语义派生与语义租约
        // （Codex 第四轮复核 #2：终态空线程必须先删除 semantic state / lease，
        // 再撤销 claims/decisions/questions/expectations）。
        let member_count = CountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT COUNT(*) AS value FROM secretary_thread_events WHERE thread_id = ?",
            [thread_id.into()],
        ))
        .one(db)
        .await
        .map_err(store_error)?
        .map(|row| row.value.max(0) as usize)
        .unwrap_or_default();
        if member_count > 0 {
            return Ok(());
        }
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "DELETE FROM secretary_thread_semantic_state WHERE thread_id = ?",
            [thread_id.into()],
        ))
        .await
        .map_err(store_error)?;
        revoke_semantic_derivations_in_txn(db, thread_id, now).await?;
        return Ok(());
    }

    let closed = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"UPDATE secretary_event_threads t
               SET t.status = 'closed', t.updated_at = ?
               WHERE t.thread_id = ?
                 AND t.status = ?
                 AND NOT EXISTS (
                     SELECT 1 FROM secretary_thread_events te
                     WHERE te.thread_id = t.thread_id
                 )"#,
            [
                now.into(),
                thread_id.into(),
                thread_row.status.clone().into(),
            ],
        ))
        .await
        .map_err(store_error)?;
    if closed.rows_affected() == 0 {
        // 并发已插入成员或状态已被语义事务迁移到终态：不关闭、不写历史。
        return Ok(());
    }

    // 线程归属已变化：清除该线程的语义批处理状态与租约（与线程变更路径先例一致）。
    // 语义 Worker 的 commit 会因租约行消失而判 LeaseLost，fencing 未提交的派生。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_thread_semantic_state WHERE thread_id = ?",
        [thread_id.into()],
    ))
    .await
    .map_err(store_error)?;
    // 撤销已提交的语义派生（Codex 复核 P1-3）：事件已迁入父线程，旧线程上的
    // claims/decisions/open questions 及其回复期待全部失效，保留状态字段作为审计；
    // 事件重新投影后由语义层在父线程重新提取，不残留指向空线程的活跃派生。
    revoke_semantic_derivations_in_txn(db, thread_id, now).await?;
    // 生命周期审计：system_recovery 权威记录空线程关闭。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"INSERT INTO secretary_thread_status_history
           (change_id, thread_id, from_status, to_status, authority, reason, created_at)
           VALUES (?, ?, ?, 'closed', 'system_recovery',
                   'reply resolution moved sole member to parent thread', ?)"#,
        [
            Uuid::new_v4().to_string().into(),
            thread_id.into(),
            thread_row.status.into(),
            now.into(),
        ],
    ))
    .await
    .map_err(store_error)?;
    Ok(())
}

/// 事务内撤销空线程上已提交的语义派生（Codex 复核 P1-3）。
///
/// 只把非终态派生迁移到各自的失效终态，保留行与来源映射作为审计；不清除来源，
/// 事件重新投影后语义层在父线程基于来源事件重新提取。
async fn revoke_semantic_derivations_in_txn(
    db: &sea_orm::DatabaseTransaction,
    thread_id: &str,
    now: chrono::NaiveDateTime,
) -> Result<(), InboundEventStoreError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"UPDATE secretary_thread_claims
           SET status = 'withdrawn', updated_at = ?
           WHERE thread_id = ? AND status IN ('proposed', 'contested', 'confirmed')"#,
        [now.into(), thread_id.into()],
    ))
    .await
    .map_err(store_error)?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"UPDATE secretary_thread_decisions
           SET status = 'revoked', updated_at = ?
           WHERE thread_id = ? AND status IN ('proposed', 'confirmed')"#,
        [now.into(), thread_id.into()],
    ))
    .await
    .map_err(store_error)?;
    // 领域规则：开放问题阻止线程关闭。空线程上的 open question 随事件迁移一并失效。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"UPDATE secretary_thread_open_questions
           SET status = 'dismissed', updated_at = ?
           WHERE thread_id = ? AND status = 'open'"#,
        [now.into(), thread_id.into()],
    ))
    .await
    .map_err(store_error)?;
    // 回复期待跟随问题失效；active -> dismissed 保留审计。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"UPDATE secretary_response_expectations
           SET expectation_status = 'dismissed', updated_at = ?
           WHERE thread_id = ? AND expectation_status = 'active'"#,
        [now.into(), thread_id.into()],
    ))
    .await
    .map_err(store_error)?;
    Ok(())
}

#[derive(Debug, FromQueryResult)]
struct ThreadStatusRow {
    status: String,
}

/// MySQL `COUNT(*)` 返回有符号 `BIGINT`；解码必须用 `i64`。
#[derive(Debug, FromQueryResult)]
struct CountRow {
    value: i64,
}

pub(super) fn store_error(error: sea_orm::DbErr) -> InboundEventStoreError {
    let _ = error;
    error!(
        error_code = "database_operation_failed",
        "personal secretary inbound store operation failed"
    );
    InboundEventStoreError::Database("database operation failed".into())
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
