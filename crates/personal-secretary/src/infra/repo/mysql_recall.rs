//! MySQL 撤回仓储：实现 [`crate::RecallStoreT`]。
//!
//! 撤回事件和 tombstone 持久化到 MySQL。不物理删除审计历史。
//! 关联键 `(account_id, channel, conversation, platform_message_id)` 禁止单 message_id 跨账号。
//! JSON 列用 `CAST(... AS CHAR)` 读取（CLAUDE.md 教训）。

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveValue::NotSet, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, FromQueryResult, QueryFilter, Set, Statement, TransactionTrait,
};
use tracing::debug;

use crate::{
    ClaimedRecallEvent, ConversationRef, MessageSource, RecallCorrelationKey, RecallEvent,
    RecallEventId, RecallFailureKind, RecallStoreError, RecallStoreT, SourceAccountRef,
    TombstoneRecord, TombstoneStatus,
};

use super::entities::{secretary_conversations, secretary_source_events};
use super::mysql_retriever::resolve_account_id;

pub(crate) struct MySqlRecallStore {
    db: DatabaseConnection,
}

impl MySqlRecallStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

/// 撤回事件行。
#[derive(sea_orm::FromQueryResult)]
#[allow(dead_code)]
struct RecallEventRow {
    recall_event_id: String,
    account_id: u64,
    recall_kind: String,
    channel: String,
    conversation_kind: String,
    platform_conversation_id: String,
    platform_message_id: String,
    correlation_key: String,
    operator_platform_id: Option<String>,
    occurred_at_unix_secs: i64,
}

/// Tombstone 行。
#[derive(sea_orm::FromQueryResult)]
#[allow(dead_code)]
struct TombstoneRow {
    source_event_id: Option<String>,
    recall_event_id: String,
    channel: String,
    conversation_kind: String,
    platform_conversation_id: String,
    platform_message_id: String,
    correlation_key: String,
    status: String,
    invalidation_reason: String,
    invalidated_at_unix_secs: i64,
    created_at_unix_secs: i64,
}

/// 把 `sea_orm::DbErr` 转为 `RecallStoreError`。
fn db_err(e: sea_orm::DbErr) -> RecallStoreError {
    RecallStoreError::from(e)
}

/// 从 `RecallCorrelationKey` 提取 SQL 参数。
struct CorrelationParts {
    channel: String,
    conv_kind: String,
    conv_id: String,
    message_id: String,
    key_string: String,
}

fn correlation_parts(c: &RecallCorrelationKey) -> CorrelationParts {
    CorrelationParts {
        channel: c.channel.as_str().to_string(),
        conv_kind: c.conversation.kind.as_str().to_string(),
        conv_id: c.conversation.id.clone(),
        message_id: c.platform_message_id.clone(),
        key_string: c.key_string(),
    }
}

/// 从行重建 `TombstoneRecord`。
async fn map_tombstone_row(
    db: &impl ConnectionTrait,
    account_id: u64,
    row: TombstoneRow,
) -> Result<TombstoneRecord, RecallStoreError> {
    let channel = match row.channel.as_str() {
        "napcat" => MessageSource::NapCat,
        "qq_open_platform" => MessageSource::QqOpenPlatform,
        _ => {
            return Err(RecallStoreError::InvalidData(format!(
                "unknown channel: {}",
                row.channel
            )));
        }
    };
    let conv_kind = match row.conversation_kind.as_str() {
        "private" => crate::ConversationKind::Private,
        "group" => crate::ConversationKind::Group,
        "owner_control" => crate::ConversationKind::OwnerControl,
        _ => {
            return Err(RecallStoreError::InvalidData(format!(
                "unknown conversation_kind: {}",
                row.conversation_kind
            )));
        }
    };
    let conv = ConversationRef::new(conv_kind, row.platform_conversation_id)
        .map_err(|e| RecallStoreError::InvalidData(e.to_string()))?;
    // 通过 account_id 反查 account_ref。
    #[derive(sea_orm::FromQueryResult)]
    struct AccountRow {
        source_channel: String,
        platform_account_id: String,
    }
    let account_row = AccountRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT source_channel, platform_account_id FROM secretary_accounts WHERE id = ?",
        [account_id.into()],
    ))
    .one(db)
    .await
    .map_err(db_err)?
    .ok_or_else(|| RecallStoreError::InvalidData(format!("account_id {account_id} not found")))?;
    let account_channel = match account_row.source_channel.as_str() {
        "napcat" => MessageSource::NapCat,
        "qq_open_platform" => MessageSource::QqOpenPlatform,
        _ => {
            return Err(RecallStoreError::InvalidData(format!(
                "unknown source_channel: {}",
                account_row.source_channel
            )));
        }
    };
    let account = SourceAccountRef::new(account_channel, account_row.platform_account_id)
        .map_err(|e| RecallStoreError::InvalidData(e.to_string()))?;
    let correlation = RecallCorrelationKey::new(account, channel, conv, row.platform_message_id)
        .map_err(|e| RecallStoreError::InvalidData(e.to_string()))?;
    let status = TombstoneStatus::parse_from_str(&row.status).ok_or_else(|| {
        RecallStoreError::InvalidData(format!("unknown tombstone status: {}", row.status))
    })?;
    let recall_event_id = RecallEventId::new(&row.recall_event_id)
        .map_err(|e| RecallStoreError::InvalidData(e.to_string()))?;
    Ok(TombstoneRecord {
        source_event_id: row.source_event_id,
        recall_event_id,
        correlation,
        status,
        invalidation_reason: row.invalidation_reason,
        invalidated_at_unix_secs: row.invalidated_at_unix_secs,
        created_at_unix_secs: row.created_at_unix_secs,
    })
}

#[async_trait]
impl RecallStoreT for MySqlRecallStore {
    async fn record_recall(
        &self,
        recall: &RecallEvent,
    ) -> Result<TombstoneStatus, RecallStoreError> {
        let account_id = resolve_account_id(&self.db, &recall.account)
            .await
            .map_err(|e| RecallStoreError::Database(e.to_string()))?;
        let parts = correlation_parts(&recall.correlation);

        let txn = self
            .db
            .begin()
            .await
            .map_err(|e| RecallStoreError::Database(e.to_string()))?;

        // 幂等：检查是否已存在相同关联键的撤回事件。
        let existing = RecallEventRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT recall_event_id, account_id, recall_kind, channel, conversation_kind,
                      platform_conversation_id, platform_message_id, correlation_key,
                      operator_platform_id, occurred_at_unix_secs
               FROM secretary_recall_events
               WHERE account_id = ? AND correlation_key = ?"#,
            [account_id.into(), parts.key_string.clone().into()],
        ))
        .one(&txn)
        .await
        .map_err(db_err)?;

        if existing.is_some() {
            // 幂等重放：相同撤回再次到达。
            txn.commit()
                .await
                .map_err(|e| RecallStoreError::Database(e.to_string()))?;
            debug!(
                recall_event_id = recall.recall_event_id.as_str(),
                "撤回事件幂等重放（已存在相同关联键）"
            );
            return Ok(TombstoneStatus::IdempotentReapply);
        }

        // 确保会话存在，供统一 SourceEvent 外键引用。
        let now = Utc::now().naive_utc();
        let conversation_id =
            ensure_conversation_for_recall(&txn, account_id, &recall.correlation, now).await?;

        // 撤回本身也是统一 SourceEvent：source_event_id 与 recall_event_id 一致。
        // platform_event_id 使用 recall UUID，避免与原消息 platform_event_id 冲突。
        let actor_platform_id = recall
            .operator_platform_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unknown-operator".into());
        secretary_source_events::Entity::insert(secretary_source_events::ActiveModel {
            source_event_id: Set(recall.recall_event_id.as_str().to_owned()),
            account_id: Set(account_id),
            conversation_id: Set(conversation_id),
            source_channel: Set(parts.channel.clone()),
            platform_event_id: Set(format!("recall:{}", recall.recall_event_id.as_str())),
            event_type: Set("recall".into()),
            actor_platform_id: Set(actor_platform_id),
            actor_kind: Set("external".into()),
            // 撤回是对既有消息的观察事实，不是 Owner 指令或助手输出。
            message_role: Set("external_observation".into()),
            occurred_at_unix_secs: Set(recall.occurred_at_unix_secs),
            reply_to_platform_event_id: Set(Some(parts.message_id.clone())),
            reply_to_event_id: Set(None),
            processing_status: Set("processed".into()),
            received_at: Set(now),
            created_at: Set(now),
        })
        .exec(&txn)
        .await
        .map_err(db_err)?;

        // 插入撤回事件投影。
        sea_orm::ConnectionTrait::execute_raw(
            &txn,
            Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_recall_events
                   (recall_event_id, account_id, recall_kind, channel, conversation_kind,
                    platform_conversation_id, platform_message_id, correlation_key,
                    operator_platform_id, occurred_at_unix_secs)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                [
                    recall.recall_event_id.as_str().into(),
                    account_id.into(),
                    recall.kind.as_str().into(),
                    parts.channel.clone().into(),
                    parts.conv_kind.clone().into(),
                    parts.conv_id.clone().into(),
                    parts.message_id.clone().into(),
                    parts.key_string.clone().into(),
                    recall
                        .operator_platform_id
                        .clone()
                        .map(sea_orm::Value::from)
                        .unwrap_or(sea_orm::Value::Bool(None)),
                    recall.occurred_at_unix_secs.into(),
                ],
            ),
        )
        .await
        .map_err(db_err)?;

        // 检查原消息是否已存在（通过关联键查找 source_event_id）。
        #[derive(sea_orm::FromQueryResult)]
        struct SourceEventIdRow {
            source_event_id: String,
        }
        let source_event = SourceEventIdRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT e.source_event_id
               FROM secretary_source_events e
               INNER JOIN secretary_conversations c ON e.conversation_id = c.id
               WHERE e.account_id = ?
                 AND c.conversation_kind = ?
                 AND c.platform_conversation_id = ?
                 AND e.platform_event_id = ?
               LIMIT 1 FOR UPDATE"#,
            [
                account_id.into(),
                parts.conv_kind.clone().into(),
                parts.conv_id.clone().into(),
                parts.message_id.clone().into(),
            ],
        ))
        .one(&txn)
        .await
        .map_err(db_err)?;

        let (status, source_event_id, invalidation_reason, invalidated_at) = match source_event {
            Some(row) => {
                // 原消息已存在：直接标记为 applied。
                (
                    TombstoneStatus::Applied,
                    Some(row.source_event_id),
                    "message recalled".to_string(),
                    recall.occurred_at_unix_secs,
                )
            }
            None => {
                // 原消息尚未到达：创建 pending tombstone。
                (
                    TombstoneStatus::Pending,
                    None,
                    "recall arrived before original message".to_string(),
                    recall.occurred_at_unix_secs,
                )
            }
        };

        // 插入或更新 tombstone（ON DUPLICATE KEY UPDATE 实现幂等）。
        let source_id_value = source_event_id
            .clone()
            .map(sea_orm::Value::from)
            .unwrap_or(sea_orm::Value::Bool(None));
        sea_orm::ConnectionTrait::execute_raw(
            &txn,
            Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_message_tombstones
                   (account_id, source_event_id, recall_event_id, channel, conversation_kind,
                    platform_conversation_id, platform_message_id, correlation_key,
                    status, invalidation_reason, invalidated_at_unix_secs)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                   ON DUPLICATE KEY UPDATE
                    source_event_id = VALUES(source_event_id),
                    status = VALUES(status),
                    invalidation_reason = VALUES(invalidation_reason),
                    invalidated_at_unix_secs = VALUES(invalidated_at_unix_secs)"#,
                [
                    account_id.into(),
                    source_id_value,
                    recall.recall_event_id.as_str().into(),
                    parts.channel.clone().into(),
                    parts.conv_kind.clone().into(),
                    parts.conv_id.clone().into(),
                    parts.message_id.clone().into(),
                    parts.key_string.clone().into(),
                    status.as_str().into(),
                    invalidation_reason.into(),
                    invalidated_at.into(),
                ],
            ),
        )
        .await
        .map_err(db_err)?;

        // 原消息已存在时，同步把对应 Artifact 标为 recalled（B6 传播）。
        if let Some(ref original_event_id) = source_event_id {
            invalidate_artifacts_for_source(&txn, original_event_id).await?;
        }

        txn.commit()
            .await
            .map_err(|e| RecallStoreError::Database(e.to_string()))?;

        debug!(
            recall_event_id = recall.recall_event_id.as_str(),
            status = status.as_str(),
            source_event_id = ?source_event_id,
            "撤回事件已记录"
        );
        Ok(status)
    }

    async fn apply_pending_tombstone(
        &self,
        correlation: &RecallCorrelationKey,
        source_event_id: &str,
    ) -> Result<Option<TombstoneRecord>, RecallStoreError> {
        let account_id = resolve_account_id(&self.db, &correlation.account)
            .await
            .map_err(|e| RecallStoreError::Database(e.to_string()))?;
        let parts = correlation_parts(correlation);

        let txn = self
            .db
            .begin()
            .await
            .map_err(|e| RecallStoreError::Database(e.to_string()))?;

        // 查找 pending tombstone。
        let row = TombstoneRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT source_event_id, recall_event_id, channel, conversation_kind,
                      platform_conversation_id, platform_message_id, correlation_key,
                      status, invalidation_reason, invalidated_at_unix_secs,
                      UNIX_TIMESTAMP(created_at) AS created_at_unix_secs
               FROM secretary_message_tombstones
               WHERE account_id = ? AND correlation_key = ? AND status = 'pending'
               LIMIT 1"#,
            [account_id.into(), parts.key_string.clone().into()],
        ))
        .one(&txn)
        .await
        .map_err(db_err)?;

        let Some(row) = row else {
            txn.commit()
                .await
                .map_err(|e| RecallStoreError::Database(e.to_string()))?;
            // 无 pending tombstone：撤回未先到。
            return Ok(None);
        };

        // 关联原消息并更新为 applied。
        sea_orm::ConnectionTrait::execute_raw(
            &txn,
            Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_message_tombstones
                   SET source_event_id = ?, status = 'applied',
                       invalidation_reason = 'original message arrived after recall',
                       invalidated_at_unix_secs = UNIX_TIMESTAMP()
                   WHERE account_id = ? AND correlation_key = ? AND status = 'pending'"#,
                [
                    source_event_id.into(),
                    account_id.into(),
                    parts.key_string.clone().into(),
                ],
            ),
        )
        .await
        .map_err(db_err)?;

        // 撤回先到、消息后到：此刻才知道原消息 SourceEvent，传播 Artifact 失效。
        invalidate_artifacts_for_source(&txn, source_event_id).await?;

        // 在 commit 之前重建记录（commit 会消耗 txn）。
        let mut record = map_tombstone_row(&txn, account_id, row).await?;

        txn.commit()
            .await
            .map_err(|e| RecallStoreError::Database(e.to_string()))?;

        record.source_event_id = Some(source_event_id.to_string());
        record.status = TombstoneStatus::Applied;
        debug!(
            source_event_id = source_event_id,
            "pending tombstone 已关联原消息并标记为 applied"
        );
        Ok(Some(record))
    }

    async fn list_pending_for_correlation(
        &self,
        correlation: &RecallCorrelationKey,
    ) -> Result<Vec<TombstoneRecord>, RecallStoreError> {
        let account_id = resolve_account_id(&self.db, &correlation.account)
            .await
            .map_err(|e| RecallStoreError::Database(e.to_string()))?;
        let parts = correlation_parts(correlation);

        let rows = TombstoneRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT source_event_id, recall_event_id, channel, conversation_kind,
                      platform_conversation_id, platform_message_id, correlation_key,
                      status, invalidation_reason, invalidated_at_unix_secs,
                      UNIX_TIMESTAMP(created_at) AS created_at_unix_secs
               FROM secretary_message_tombstones
               WHERE account_id = ? AND correlation_key = ? AND status = 'pending'"#,
            [account_id.into(), parts.key_string.clone().into()],
        ))
        .all(&self.db)
        .await
        .map_err(db_err)?;

        let mut records = Vec::new();
        for row in rows {
            records.push(map_tombstone_row(&self.db, account_id, row).await?);
        }
        Ok(records)
    }

    async fn is_recalled(
        &self,
        account_id: u64,
        source_event_id: &str,
    ) -> Result<bool, RecallStoreError> {
        #[derive(sea_orm::FromQueryResult)]
        struct CountRow {
            cnt: i64,
        }
        let row = CountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT COUNT(*) AS cnt
               FROM secretary_message_tombstones
               WHERE account_id = ? AND source_event_id = ? AND status = 'applied'"#,
            [account_id.into(), source_event_id.into()],
        ))
        .one(&self.db)
        .await
        .map_err(db_err)?;

        Ok(row.map(|r| r.cnt > 0).unwrap_or(false))
    }

    async fn list_recalled_event_ids(
        &self,
        account_id: u64,
    ) -> Result<Vec<String>, RecallStoreError> {
        #[derive(sea_orm::FromQueryResult)]
        struct EventIdRow {
            source_event_id: String,
        }
        let rows = EventIdRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT source_event_id
               FROM secretary_message_tombstones
               WHERE account_id = ? AND status = 'applied' AND source_event_id IS NOT NULL"#,
            [account_id.into()],
        ))
        .all(&self.db)
        .await
        .map_err(db_err)?;

        Ok(rows.into_iter().map(|r| r.source_event_id).collect())
    }

    async fn enqueue_recall(&self, recall: &RecallEvent) -> Result<(), RecallStoreError> {
        let account_id = ensure_account_for_inbox(&self.db, &recall.account).await?;
        let event_json = serde_json::to_string(recall)
            .map_err(|error| RecallStoreError::InvalidData(error.to_string()))?;
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_recall_inbox
                   (recall_event_id, account_id, correlation_key, event_json)
                   VALUES (?, ?, ?, CAST(? AS JSON))
                   ON DUPLICATE KEY UPDATE recall_event_id = recall_event_id"#,
                [
                    recall.recall_event_id.as_str().into(),
                    account_id.into(),
                    recall.correlation.key_string().into(),
                    event_json.into(),
                ],
            ))
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn claim_recall(
        &self,
        lease_secs: u64,
    ) -> Result<Option<ClaimedRecallEvent>, RecallStoreError> {
        if !(1..=3600).contains(&lease_secs) {
            return Err(RecallStoreError::InvalidData(
                "recall lease_secs must be in 1..=3600".into(),
            ));
        }
        #[derive(FromQueryResult)]
        struct InboxRow {
            recall_event_id: String,
            event_json: String,
            attempts: u32,
        }
        let txn = self.db.begin().await.map_err(db_err)?;
        txn.execute_raw(Statement::from_string(
            DatabaseBackend::MySql,
            r#"UPDATE secretary_recall_inbox
               SET status = 'pending', lease_token = NULL, lease_expires_at = NULL,
                   next_attempt_at = UTC_TIMESTAMP(6), last_error_code = 'lease_expired'
               WHERE status = 'claimed' AND lease_expires_at < UTC_TIMESTAMP(6)"#,
        ))
        .await
        .map_err(db_err)?;
        let row = InboxRow::find_by_statement(Statement::from_string(
            DatabaseBackend::MySql,
            r#"SELECT recall_event_id, CAST(event_json AS CHAR) AS event_json, attempts
               FROM secretary_recall_inbox
               WHERE status = 'pending' AND next_attempt_at <= UTC_TIMESTAMP(6)
               ORDER BY created_at, recall_event_id
               LIMIT 1 FOR UPDATE SKIP LOCKED"#,
        ))
        .one(&txn)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            txn.commit().await.map_err(db_err)?;
            return Ok(None);
        };
        let lease_token = uuid::Uuid::new_v4().to_string();
        let result = txn
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_recall_inbox
                   SET status = 'claimed', attempts = attempts + 1, lease_token = ?,
                       lease_expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND)
                   WHERE recall_event_id = ? AND status = 'pending'"#,
                [
                    lease_token.clone().into(),
                    lease_secs.into(),
                    row.recall_event_id.clone().into(),
                ],
            ))
            .await
            .map_err(db_err)?;
        if result.rows_affected() != 1 {
            return Err(RecallStoreError::Database("recall inbox lease lost".into()));
        }
        txn.commit().await.map_err(db_err)?;
        let event = serde_json::from_str(&row.event_json)
            .map_err(|error| RecallStoreError::InvalidData(error.to_string()))?;
        Ok(Some(ClaimedRecallEvent {
            event,
            lease_token,
            attempt: row.attempts.saturating_add(1),
        }))
    }

    async fn mark_recall_applied(
        &self,
        recall_event_id: &str,
        lease_token: &str,
    ) -> Result<(), RecallStoreError> {
        update_inbox_status(
            &self.db,
            recall_event_id,
            lease_token,
            "applied",
            None,
            false,
        )
        .await
    }

    async fn mark_recall_failed(
        &self,
        recall_event_id: &str,
        lease_token: &str,
        error_code: &str,
        kind: RecallFailureKind,
    ) -> Result<(), RecallStoreError> {
        let retry = kind == RecallFailureKind::Retryable;
        update_inbox_status(
            &self.db,
            recall_event_id,
            lease_token,
            if retry { "pending" } else { "failed" },
            Some(error_code),
            retry,
        )
        .await
    }
}

async fn ensure_account_for_inbox(
    db: &DatabaseConnection,
    account: &SourceAccountRef,
) -> Result<u64, RecallStoreError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"INSERT INTO secretary_accounts (source_channel, platform_account_id, status)
           VALUES (?, ?, 'active')
           ON DUPLICATE KEY UPDATE updated_at = UTC_TIMESTAMP(6)"#,
        [
            account.channel.as_str().into(),
            account.account_id.clone().into(),
        ],
    ))
    .await
    .map_err(db_err)?;
    resolve_account_id(db, account)
        .await
        .map_err(|error| RecallStoreError::Database(error.to_string()))
}

async fn update_inbox_status(
    db: &DatabaseConnection,
    recall_event_id: &str,
    lease_token: &str,
    status: &str,
    error_code: Option<&str>,
    retry: bool,
) -> Result<(), RecallStoreError> {
    if error_code.is_some_and(|value| value.len() > 64) {
        return Err(RecallStoreError::InvalidData(
            "recall inbox error_code exceeds 64 bytes".into(),
        ));
    }
    let sql = if retry {
        r#"UPDATE secretary_recall_inbox
           SET status = ?, last_error_code = ?, lease_token = NULL, lease_expires_at = NULL,
               next_attempt_at = DATE_ADD(
                   UTC_TIMESTAMP(6),
                   INTERVAL LEAST(300, POW(2, LEAST(attempts, 8))) SECOND
               )
           WHERE recall_event_id = ? AND status = 'claimed' AND lease_token = ?"#
    } else {
        r#"UPDATE secretary_recall_inbox
           SET status = ?, last_error_code = ?, lease_token = NULL, lease_expires_at = NULL
           WHERE recall_event_id = ? AND status = 'claimed' AND lease_token = ?"#
    };
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            [
                status.into(),
                error_code
                    .map(sea_orm::Value::from)
                    .unwrap_or(sea_orm::Value::Bool(None)),
                recall_event_id.into(),
                lease_token.into(),
            ],
        ))
        .await
        .map_err(db_err)?;
    if result.rows_affected() != 1 {
        return Err(RecallStoreError::Database("recall inbox lease lost".into()));
    }
    Ok(())
}

/// 为撤回事件确保会话行存在，返回 conversation_id。
async fn ensure_conversation_for_recall(
    db: &impl ConnectionTrait,
    account_id: u64,
    correlation: &RecallCorrelationKey,
    now: chrono::NaiveDateTime,
) -> Result<u64, RecallStoreError> {
    use sea_orm::sea_query::OnConflict;

    let model = secretary_conversations::ActiveModel {
        id: NotSet,
        account_id: Set(account_id),
        conversation_kind: Set(correlation.conversation.kind.as_str().into()),
        platform_conversation_id: Set(correlation.conversation.id.clone()),
        memory_mode: Set("normal".into()),
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
        .map_err(db_err)?;

    let stored = secretary_conversations::Entity::find()
        .filter(secretary_conversations::Column::AccountId.eq(account_id))
        .filter(
            secretary_conversations::Column::ConversationKind
                .eq(correlation.conversation.kind.as_str()),
        )
        .filter(
            secretary_conversations::Column::PlatformConversationId
                .eq(correlation.conversation.id.clone()),
        )
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| RecallStoreError::Database("conversation vanished after ensure".into()))?;
    Ok(stored.id)
}

/// 在同一事务内把原消息的 Artifact 标记为 recalled。
async fn invalidate_artifacts_for_source(
    db: &impl ConnectionTrait,
    source_event_id: &str,
) -> Result<(), RecallStoreError> {
    sea_orm::ConnectionTrait::execute_raw(
        db,
        Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"UPDATE secretary_artifacts
               SET availability = 'recalled'
               WHERE source_event_id = ? AND availability = 'available'"#,
            [source_event_id.into()],
        ),
    )
    .await
    .map_err(db_err)?;
    Ok(())
}
