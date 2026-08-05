use async_trait::async_trait;
use chrono::{Duration, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use crate::{
    ClaimedThreadProjectionBatch, ConversationKind, ConversationRef, EventThreadId,
    InboundEventStoreError, MessageSource, SourceAccountRef, SourceEventId, ThreadContextEvent,
    ThreadProjectionEvent, ThreadProjectionLeaseToken, ThreadProjectionPlan,
    ThreadProjectionStoreT,
};

use super::mysql_inbound::store_error;

pub(crate) struct MySqlThreadProjectionStore {
    db: DatabaseConnection,
}

impl MySqlThreadProjectionStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ThreadProjectionStoreT for MySqlThreadProjectionStore {
    async fn claim_projection_batch(
        &self,
        max_events: u32,
        lease_secs: u64,
        same_conversation_window_secs: i64,
    ) -> Result<Option<ClaimedThreadProjectionBatch>, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let lease_expires_at = now + Duration::seconds(lease_secs as i64);
        let lease_token = ThreadProjectionLeaseToken::new(Uuid::new_v4().to_string())
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;

        let rows = ProjectionEventRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"
SELECT e.source_event_id,
       a.source_channel,
       a.platform_account_id,
       c.conversation_kind,
       c.platform_conversation_id,
       e.actor_platform_id,
       e.occurred_at_unix_secs,
       e.reply_to_event_id,
       e.conversation_id
FROM secretary_source_events e
JOIN secretary_accounts a ON a.id = e.account_id
JOIN secretary_conversations c ON c.id = e.conversation_id
LEFT JOIN secretary_thread_events te ON te.source_event_id = e.source_event_id
LEFT JOIN secretary_thread_projection_claims pc ON pc.source_event_id = e.source_event_id
WHERE te.source_event_id IS NULL
  AND (pc.source_event_id IS NULL OR pc.lease_token IS NULL OR pc.lease_expires_at < ?)
ORDER BY e.occurred_at_unix_secs ASC, e.source_event_id ASC
LIMIT ?
FOR UPDATE SKIP LOCKED
"#,
            [now.into(), max_events.into()],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?;

        if rows.is_empty() {
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        }

        for row in &rows {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
INSERT INTO secretary_thread_projection_claims
    (source_event_id, lease_token, lease_expires_at, attempts, last_error, updated_at)
VALUES (?, ?, ?, 1, NULL, ?)
ON DUPLICATE KEY UPDATE
    lease_token = VALUES(lease_token),
    lease_expires_at = VALUES(lease_expires_at),
    attempts = attempts + 1,
    last_error = NULL,
    updated_at = VALUES(updated_at)
"#,
                    [
                        row.source_event_id.clone().into(),
                        lease_token.as_str().into(),
                        lease_expires_at.into(),
                        now.into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
        }

        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let (reply_parent_thread_id, reply_parent_thread_is_terminal) =
                optional_thread_context(
                    &transaction,
                    r#"SELECT te.thread_id AS value, et.status
FROM secretary_effective_thread_events te
JOIN secretary_event_threads et ON et.thread_id = te.thread_id
WHERE te.source_event_id = ?"#,
                    row.reply_to_event_id.as_deref(),
                )
                .await?;
            let (reply_child_thread_id, reply_child_thread_is_terminal) = optional_thread_context(
                &transaction,
                r#"SELECT te.thread_id AS value, et.status
FROM secretary_source_events child
JOIN secretary_effective_thread_events te ON te.source_event_id = child.source_event_id
JOIN secretary_event_threads et ON et.thread_id = te.thread_id
WHERE child.reply_to_event_id = ?
ORDER BY child.occurred_at_unix_secs ASC, child.source_event_id ASC
LIMIT 1"#,
                Some(&row.source_event_id),
            )
            .await?;
            let previous = PreviousEventRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"
SELECT previous.source_event_id,
       te.thread_id,
       et.status AS thread_status,
       previous.actor_platform_id,
       previous.occurred_at_unix_secs
FROM secretary_source_events previous
JOIN secretary_effective_thread_events te ON te.source_event_id = previous.source_event_id
JOIN secretary_event_threads et ON et.thread_id = te.thread_id
WHERE previous.conversation_id = ?
  AND (previous.occurred_at_unix_secs < ?
       OR (previous.occurred_at_unix_secs = ? AND previous.source_event_id < ?))
  AND previous.occurred_at_unix_secs >= ?
ORDER BY previous.occurred_at_unix_secs DESC, previous.source_event_id DESC
LIMIT 1
"#,
                [
                    row.conversation_id.into(),
                    row.occurred_at_unix_secs.into(),
                    row.occurred_at_unix_secs.into(),
                    row.source_event_id.clone().into(),
                    row.occurred_at_unix_secs
                        .saturating_sub(same_conversation_window_secs)
                        .into(),
                ],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?
            .map(previous_context)
            .transpose()?;
            let (previous, previous_thread_is_terminal) = match previous {
                Some((ctx, term)) => (Some(ctx), term),
                None => (None, false),
            };

            events.push(ThreadProjectionEvent {
                source_event_id: SourceEventId::new(row.source_event_id)?,
                account: SourceAccountRef::new(
                    parse_source(&row.source_channel)?,
                    row.platform_account_id,
                )
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                conversation: ConversationRef::new(
                    parse_conversation(&row.conversation_kind)?,
                    row.platform_conversation_id,
                )
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                actor_id: row.actor_platform_id,
                occurred_at_unix_secs: row.occurred_at_unix_secs,
                reply_to_event_id: row.reply_to_event_id.map(SourceEventId::new).transpose()?,
                reply_parent_thread_id,
                reply_child_thread_id,
                previous_in_conversation: previous,
                reply_parent_thread_is_terminal,
                reply_child_thread_is_terminal,
                previous_thread_is_terminal,
            });
        }

        transaction.commit().await.map_err(store_error)?;
        tracing::debug!(
            lease_token = %lease_token.as_str(),
            event_count = events.len(),
            lease_secs,
            "已领取个人秘书线程投影批次"
        );
        Ok(Some(ClaimedThreadProjectionBatch {
            lease_token,
            events,
        }))
    }

    async fn commit_projection(
        &self,
        plan: &ThreadProjectionPlan,
    ) -> Result<(), InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let owned = CountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"
SELECT COUNT(*) AS value
FROM secretary_thread_projection_claims
WHERE lease_token = ? AND lease_expires_at >= ?
FOR UPDATE
"#,
            [plan.lease_token.as_str().into(), now.into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        .map(|row| row.value as usize)
        .unwrap_or_default();
        if owned != plan.assignments.len() {
            return Err(InboundEventStoreError::LeaseLost);
        }

        for assignment in &plan.assignments {
            if assignment.creates_thread {
                let account = AccountIdRow::find_by_statement(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
SELECT id
FROM secretary_accounts
WHERE source_channel = ? AND platform_account_id = ?
"#,
                    [
                        assignment.account.channel.as_str().into(),
                        assignment.account.account_id.clone().into(),
                    ],
                ))
                .one(&transaction)
                .await
                .map_err(store_error)?
                .ok_or_else(|| {
                    InboundEventStoreError::InvalidData(
                        "thread assignment references a missing account".into(),
                    )
                })?;
                transaction
                    .execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::MySql,
                        r#"
INSERT INTO secretary_event_threads
    (thread_id, account_id, status, root_event_id, latest_event_id,
     opened_at_unix_secs, latest_occurred_at_unix_secs, created_at, updated_at)
VALUES (?, ?, 'open', ?, ?, ?, ?, ?, ?)
"#,
                        [
                            assignment.thread_id.as_str().into(),
                            account.id.into(),
                            assignment.root_event_id.as_str().into(),
                            assignment.source_event_id.as_str().into(),
                            assignment.occurred_at_unix_secs.into(),
                            assignment.occurred_at_unix_secs.into(),
                            now.into(),
                            now.into(),
                        ],
                    ))
                    .await
                    .map_err(store_error)?;
            } else {
                // 并发防护（Codex 复核 P1-2）：Reply 解析事务可能在并发关闭旧线程
                // （条件 UPDATE 锁定线程行）。这里先以 FOR UPDATE 锁定线程行并复验
                // 状态：已关闭/终态的线程不得再接纳成员，否则产生"closed 但非空"。
                // 锁持有到本事务提交，与解析事务的关闭 UPDATE 互斥，两种顺序都收敛
                // （先锁则成员先落库、关闭复验失败；后锁则读到 closed 跳过插入，
                // 事件保持未投影状态，claim 删除后由下次领取重新规划到父线程）。
                let status = StatusRow::find_by_statement(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "SELECT status FROM secretary_event_threads WHERE thread_id = ? FOR UPDATE",
                    [assignment.thread_id.as_str().into()],
                ))
                .one(&transaction)
                .await
                .map_err(store_error)?;
                if matches!(
                    status.as_ref().map(|row| row.status.as_str()),
                    None | Some("closed" | "resolved")
                ) {
                    // 目标线程已终态或消失：整批计划作废，不清除 claims（事件由下次
                    // 领取重新规划到父线程）。部分提交会导致已迁出事件永远无法投影
                    // 且 relation 写入指向不存在或终态线程（Codex 第三轮复核 P1-3）。
                    let _ = transaction.rollback().await;
                    return Err(InboundEventStoreError::LeaseLost);
                }
            }

            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
INSERT INTO secretary_thread_events (source_event_id, thread_id, added_at)
VALUES (?, ?, ?)
"#,
                    [
                        assignment.source_event_id.as_str().into(),
                        assignment.thread_id.as_str().into(),
                        now.into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
UPDATE secretary_event_threads
SET latest_event_id = IF(latest_occurred_at_unix_secs <= ?, ?, latest_event_id),
    latest_occurred_at_unix_secs = GREATEST(latest_occurred_at_unix_secs, ?),
    updated_at = ?
WHERE thread_id = ?
"#,
                    [
                        assignment.occurred_at_unix_secs.into(),
                        assignment.source_event_id.as_str().into(),
                        assignment.occurred_at_unix_secs.into(),
                        now.into(),
                        assignment.thread_id.as_str().into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
        }

        for relation in &plan.relations {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
INSERT IGNORE INTO secretary_thread_relations
    (relation_id, thread_id, from_event_id, to_event_id, relation_kind,
     confidence_bps, reason, created_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?)
"#,
                    [
                        relation.relation_id.as_str().into(),
                        relation.thread_id.as_str().into(),
                        relation.from_event_id.as_str().into(),
                        relation.to_event_id.as_str().into(),
                        relation.kind.as_str().into(),
                        relation.confidence_bps.into(),
                        relation.reason.clone().into(),
                        now.into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
        }

        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "DELETE FROM secretary_thread_projection_claims WHERE lease_token = ?",
                [plan.lease_token.as_str().into()],
            ))
            .await
            .map_err(store_error)?;
        transaction.commit().await.map_err(store_error)?;
        tracing::debug!(
            lease_token = %plan.lease_token.as_str(),
            events_projected = plan.assignments.len(),
            relations_created = plan.relations.len(),
            "个人秘书确定性线程投影事务已提交"
        );
        Ok(())
    }

    async fn release_projection_claims(
        &self,
        lease_token: &ThreadProjectionLeaseToken,
        error: &str,
    ) -> Result<(), InboundEventStoreError> {
        let safe_error: String = error.chars().take(512).collect();
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"
UPDATE secretary_thread_projection_claims
SET lease_token = NULL, lease_expires_at = NULL, last_error = ?, updated_at = ?
WHERE lease_token = ?
"#,
                [
                    safe_error.into(),
                    Utc::now().naive_utc().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        Ok(())
    }
}

/// 查询线程上下文（thread_id + status），用于判定父/子线程是否已终态。
/// 返回 `(Option<EventThreadId>, bool)` 其中 bool 为终态标记
/// （Codex 第四轮复核 #4）。
async fn optional_thread_context<C: ConnectionTrait>(
    db: &C,
    sql: &str,
    event_id: Option<&str>,
) -> Result<(Option<EventThreadId>, bool), InboundEventStoreError> {
    let Some(event_id) = event_id else {
        return Ok((None, false));
    };
    let row = ThreadContextRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        sql,
        [event_id.into()],
    ))
    .one(db)
    .await
    .map_err(store_error)?;
    match row {
        Some(row) => {
            let thread_id = EventThreadId::new(row.value)
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
            let is_terminal = is_terminal_thread_status(&row.status);
            Ok((Some(thread_id), is_terminal))
        }
        None => Ok((None, false)),
    }
}

fn is_terminal_thread_status(status: &str) -> bool {
    matches!(status, "resolved" | "closed")
}

fn previous_context(
    row: PreviousEventRow,
) -> Result<(ThreadContextEvent, bool), InboundEventStoreError> {
    let is_terminal = is_terminal_thread_status(&row.thread_status);
    Ok((
        ThreadContextEvent {
            source_event_id: SourceEventId::new(row.source_event_id)?,
            thread_id: EventThreadId::new(row.thread_id)
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
            actor_id: row.actor_platform_id,
            occurred_at_unix_secs: row.occurred_at_unix_secs,
        },
        is_terminal,
    ))
}

fn parse_source(value: &str) -> Result<MessageSource, InboundEventStoreError> {
    match value {
        "napcat" => Ok(MessageSource::NapCat),
        "qq_open_platform" => Ok(MessageSource::QqOpenPlatform),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown source channel {value}"
        ))),
    }
}

fn parse_conversation(value: &str) -> Result<ConversationKind, InboundEventStoreError> {
    match value {
        "private" => Ok(ConversationKind::Private),
        "group" => Ok(ConversationKind::Group),
        "owner_control" => Ok(ConversationKind::OwnerControl),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown conversation kind {value}"
        ))),
    }
}

#[derive(Debug, FromQueryResult)]
struct ProjectionEventRow {
    source_event_id: String,
    source_channel: String,
    platform_account_id: String,
    conversation_kind: String,
    platform_conversation_id: String,
    actor_platform_id: String,
    occurred_at_unix_secs: i64,
    reply_to_event_id: Option<String>,
    conversation_id: u64,
}

#[derive(Debug, FromQueryResult)]
struct PreviousEventRow {
    source_event_id: String,
    thread_id: String,
    thread_status: String,
    actor_platform_id: String,
    occurred_at_unix_secs: i64,
}

/// 线程上下文查询结果（thread_id + status），用于判定父/子线程是否已终态
/// （Codex 第四轮复核 #4）。
#[derive(Debug, FromQueryResult)]
struct ThreadContextRow {
    value: String,
    status: String,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    value: i64,
}

#[derive(Debug, FromQueryResult)]
struct AccountIdRow {
    id: u64,
}

#[derive(Debug, FromQueryResult)]
struct StatusRow {
    status: String,
}
