use async_trait::async_trait;
use chrono::{Duration, NaiveDateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use crate::{
    ClaimedThreadSemanticBatch, EventThreadId, InboundEventStoreError, MessageRole, MessageSource,
    OpenQuestionId, SourceAccountRef, SourceEventId, ThreadActorRef, ThreadDecisionId,
    ThreadSemanticCursor, ThreadSemanticEvent, ThreadSemanticLeaseToken, ThreadSemanticPatch,
    ThreadSemanticStoreT, ThreadStatus, validate_semantic_patch,
};

use super::mysql_inbound::store_error;

pub(crate) struct MySqlThreadSemanticStore {
    db: DatabaseConnection,
}

impl MySqlThreadSemanticStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ThreadSemanticStoreT for MySqlThreadSemanticStore {
    async fn claim_semantic_batch(
        &self,
        max_events: u32,
        max_total_chars: u32,
        lease_secs: u64,
    ) -> Result<Option<ClaimedThreadSemanticBatch>, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let thread = SemanticThreadRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"
SELECT t.thread_id, t.status, s.last_added_at, s.last_source_event_id
FROM secretary_event_threads t
LEFT JOIN secretary_thread_semantic_state s ON s.thread_id = t.thread_id
WHERE (s.thread_id IS NULL OR s.lease_token IS NULL OR s.lease_expires_at < ?)
  AND EXISTS (
      SELECT 1
      FROM secretary_effective_thread_events te
      JOIN secretary_source_events e ON e.source_event_id = te.source_event_id
      JOIN secretary_conversations c ON c.id = e.conversation_id
      JOIN secretary_message_contents mc ON mc.source_event_id = e.source_event_id
      WHERE te.thread_id = t.thread_id
        AND c.memory_mode IN ('normal', 'local_only')
        AND mc.content_mode IN ('normal', 'local_only')
        AND (s.last_added_at IS NULL
             OR te.added_at > s.last_added_at
             OR (te.added_at = s.last_added_at AND te.source_event_id > s.last_source_event_id))
  )
ORDER BY t.updated_at ASC, t.thread_id ASC
LIMIT 1
FOR UPDATE SKIP LOCKED
"#,
            [now.into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?;
        let Some(thread) = thread else {
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        };

        let lease_token = ThreadSemanticLeaseToken::new(Uuid::new_v4().to_string())
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        let lease_expires_at = now + Duration::seconds(lease_secs as i64);
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"
INSERT INTO secretary_thread_semantic_state
    (thread_id, last_added_at, last_source_event_id, lease_token, lease_expires_at,
     attempts, last_error, updated_at)
VALUES (?, NULL, NULL, ?, ?, 1, NULL, ?)
ON DUPLICATE KEY UPDATE
    lease_token = VALUES(lease_token),
    lease_expires_at = VALUES(lease_expires_at),
    attempts = attempts + 1,
    last_error = NULL,
    updated_at = VALUES(updated_at)
"#,
                [
                    thread.thread_id.clone().into(),
                    lease_token.as_str().into(),
                    lease_expires_at.into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;

        let rows = SemanticEventRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"
SELECT te.source_event_id, te.added_at,
       a.source_channel, a.platform_account_id,
       e.actor_platform_id, e.message_role, e.occurred_at_unix_secs,
       mc.normalized_text
FROM secretary_effective_thread_events te
JOIN secretary_source_events e ON e.source_event_id = te.source_event_id
JOIN secretary_accounts a ON a.id = e.account_id
JOIN secretary_conversations c ON c.id = e.conversation_id
JOIN secretary_message_contents mc ON mc.source_event_id = e.source_event_id
WHERE te.thread_id = ?
  AND c.memory_mode IN ('normal', 'local_only')
  AND mc.content_mode IN ('normal', 'local_only')
  AND (? IS NULL
       OR te.added_at > ?
       OR (te.added_at = ? AND te.source_event_id > ?))
ORDER BY te.added_at ASC, te.source_event_id ASC
LIMIT ?
"#,
            [
                thread.thread_id.clone().into(),
                thread.last_added_at.into(),
                thread.last_added_at.into(),
                thread.last_added_at.into(),
                thread.last_source_event_id.clone().into(),
                max_events.into(),
            ],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?;
        let last = rows.last().ok_or_else(|| {
            InboundEventStoreError::InvalidData(
                "claimed semantic thread did not contain a readable event".into(),
            )
        })?;
        let next_cursor = ThreadSemanticCursor {
            added_at_unix_micros: last.added_at.and_utc().timestamp_micros(),
            source_event_id: SourceEventId::new(last.source_event_id.clone())?,
        };
        let mut remaining_chars = max_total_chars as usize;
        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let char_count = row.normalized_text.chars().count();
            let content_omitted = char_count > remaining_chars;
            let normalized_text = if content_omitted {
                String::new()
            } else {
                remaining_chars -= char_count;
                row.normalized_text
            };
            let account =
                SourceAccountRef::new(parse_source(&row.source_channel)?, row.platform_account_id)
                    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
            events.push(ThreadSemanticEvent {
                source_event_id: SourceEventId::new(row.source_event_id)?,
                actor: ThreadActorRef {
                    account,
                    actor_id: row.actor_platform_id,
                },
                role: parse_role(&row.message_role)?,
                occurred_at_unix_secs: row.occurred_at_unix_secs,
                normalized_text,
                content_omitted,
            });
        }

        let confirmed_decision_ids = ValueRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT decision.decision_id AS value FROM secretary_thread_decisions decision \
             WHERE decision.thread_id = ? AND decision.status = 'confirmed' \
             AND NOT EXISTS (SELECT 1 FROM secretary_thread_semantic_invalidations invalidation \
                 WHERE invalidation.thread_id = decision.thread_id \
                 AND invalidation.created_at >= decision.updated_at) \
             ORDER BY decision.created_at, decision.decision_id",
            [thread.thread_id.clone().into()],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?
        .into_iter()
        .map(|row| {
            ThreadDecisionId::new(row.value)
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
        let open_question_ids = ValueRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT question.question_id AS value FROM secretary_thread_open_questions question \
             WHERE question.thread_id = ? AND question.status = 'open' \
             AND NOT EXISTS (SELECT 1 FROM secretary_thread_semantic_invalidations invalidation \
                 WHERE invalidation.thread_id = question.thread_id \
                 AND invalidation.created_at >= question.updated_at) \
             ORDER BY question.created_at, question.question_id",
            [thread.thread_id.clone().into()],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?
        .into_iter()
        .map(|row| {
            OpenQuestionId::new(row.value)
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;

        transaction.commit().await.map_err(store_error)?;
        tracing::debug!(
            thread_id = %thread.thread_id,
            lease_token = %lease_token.as_str(),
            events = events.len(),
            omitted_events = events.iter().filter(|event| event.content_omitted).count(),
            "已领取有界线程语义批次"
        );
        Ok(Some(ClaimedThreadSemanticBatch {
            lease_token,
            thread_id: EventThreadId::new(thread.thread_id)
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
            current_status: parse_status(&thread.status)?,
            confirmed_decision_ids,
            open_question_ids,
            events,
            next_cursor,
        }))
    }

    async fn commit_semantic_patch(
        &self,
        batch: &ClaimedThreadSemanticBatch,
        patch: &ThreadSemanticPatch,
    ) -> Result<(), InboundEventStoreError> {
        validate_semantic_patch(batch, patch)
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let owned = CountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT COUNT(*) AS value FROM secretary_thread_semantic_state \
             WHERE thread_id = ? AND lease_token = ? AND lease_expires_at >= ? FOR UPDATE",
            [
                batch.thread_id.as_str().into(),
                batch.lease_token.as_str().into(),
                now.into(),
            ],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        .map(|row| row.value)
        .unwrap_or_default();
        if owned != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }

        for claim in &patch.claims {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
INSERT INTO secretary_thread_claims
    (claim_id, thread_id, claim_kind, claimant_channel, claimant_account,
     claimant_actor_id, statement, status, confidence_bps, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, 'proposed', ?, ?, ?)
"#,
                    [
                        claim.claim_id.as_str().into(),
                        claim.thread_id.as_str().into(),
                        claim.kind.as_str().into(),
                        claim.claimant.account.channel.as_str().into(),
                        claim.claimant.account.account_id.clone().into(),
                        claim.claimant.actor_id.clone().into(),
                        claim.statement.clone().into(),
                        claim.confidence_bps.into(),
                        now.into(),
                        now.into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
            insert_sources(
                &transaction,
                "secretary_thread_claim_sources",
                "claim_id",
                claim.claim_id.as_str(),
                &claim.source_event_ids,
            )
            .await?;
        }
        for decision in &patch.decisions {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
INSERT INTO secretary_thread_decisions
    (decision_id, thread_id, statement, status, confidence_bps, supersedes_id,
     created_at, updated_at)
VALUES (?, ?, ?, 'proposed', ?, ?, ?, ?)
"#,
                    [
                        decision.decision_id.as_str().into(),
                        decision.thread_id.as_str().into(),
                        decision.statement.clone().into(),
                        decision.confidence_bps.into(),
                        decision.supersedes.as_ref().map(|id| id.as_str()).into(),
                        now.into(),
                        now.into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
            insert_sources(
                &transaction,
                "secretary_thread_decision_sources",
                "decision_id",
                decision.decision_id.as_str(),
                &decision.source_event_ids,
            )
            .await?;
        }
        for question in &patch.questions {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
INSERT INTO secretary_thread_open_questions
    (question_id, thread_id, raised_by_channel, raised_by_account, raised_by_actor_id,
     question, status, confidence_bps, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, 'open', ?, ?, ?)
"#,
                    [
                        question.question_id.as_str().into(),
                        question.thread_id.as_str().into(),
                        question.raised_by.account.channel.as_str().into(),
                        question.raised_by.account.account_id.clone().into(),
                        question.raised_by.actor_id.clone().into(),
                        question.question.clone().into(),
                        question.confidence_bps.into(),
                        now.into(),
                        now.into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
            insert_sources(
                &transaction,
                "secretary_thread_question_sources",
                "question_id",
                question.question_id.as_str(),
                &question.source_event_ids,
            )
            .await?;
        }
        if let Some(change) = &patch.lifecycle_change {
            let updated = transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "UPDATE secretary_event_threads SET status = ?, updated_at = ? \
                     WHERE thread_id = ? AND status = ?",
                    [
                        change.to.as_str().into(),
                        now.into(),
                        change.thread_id.as_str().into(),
                        change.from.as_str().into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
            if updated.rows_affected() != 1 {
                return Err(InboundEventStoreError::LeaseLost);
            }
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
INSERT INTO secretary_thread_status_history
    (change_id, thread_id, from_status, to_status, authority, reason, created_at)
VALUES (?, ?, ?, ?, ?, ?, ?)
"#,
                    [
                        change.change_id.as_str().into(),
                        change.thread_id.as_str().into(),
                        change.from.as_str().into(),
                        change.to.as_str().into(),
                        change.authority.as_str().into(),
                        change.reason.clone().into(),
                        now.into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
            insert_sources(
                &transaction,
                "secretary_thread_status_sources",
                "change_id",
                change.change_id.as_str(),
                &change.source_event_ids,
            )
            .await?;
        }

        let cursor_time =
            chrono::DateTime::from_timestamp_micros(batch.next_cursor.added_at_unix_micros)
                .ok_or_else(|| {
                    InboundEventStoreError::InvalidData("invalid semantic cursor time".into())
                })?
                .naive_utc();
        let updated = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"
UPDATE secretary_thread_semantic_state
SET last_added_at = ?, last_source_event_id = ?, lease_token = NULL,
    lease_expires_at = NULL, last_error = NULL, updated_at = ?
WHERE thread_id = ? AND lease_token = ?
"#,
                [
                    cursor_time.into(),
                    batch.next_cursor.source_event_id.as_str().into(),
                    now.into(),
                    batch.thread_id.as_str().into(),
                    batch.lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if updated.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        transaction.commit().await.map_err(store_error)?;
        tracing::debug!(
            thread_id = %batch.thread_id.as_str(),
            events = batch.events.len(),
            claims = patch.claims.len(),
            decisions = patch.decisions.len(),
            questions = patch.questions.len(),
            lifecycle_changed = patch.lifecycle_change.is_some(),
            "线程类型化语义补丁已原子提交"
        );
        Ok(())
    }

    async fn release_semantic_claim(
        &self,
        lease_token: &ThreadSemanticLeaseToken,
        error: &str,
    ) -> Result<(), InboundEventStoreError> {
        let safe_error: String = error.chars().take(512).collect();
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_thread_semantic_state SET lease_token = NULL, \
                 lease_expires_at = NULL, last_error = ?, updated_at = ? WHERE lease_token = ?",
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

async fn insert_sources(
    db: &sea_orm::DatabaseTransaction,
    table: &str,
    id_column: &str,
    entity_id: &str,
    sources: &[SourceEventId],
) -> Result<(), InboundEventStoreError> {
    // 表名和列名只由本模块内固定调用点提供，不接收外部输入。
    let sql = format!("INSERT INTO {table} ({id_column}, source_event_id) VALUES (?, ?)");
    for source in sources {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            &sql,
            [entity_id.into(), source.as_str().into()],
        ))
        .await
        .map_err(store_error)?;
    }
    Ok(())
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

fn parse_role(value: &str) -> Result<MessageRole, InboundEventStoreError> {
    match value {
        "owner_command" => Ok(MessageRole::OwnerCommand),
        "owner_observation" => Ok(MessageRole::OwnerObservation),
        "external_observation" => Ok(MessageRole::ExternalObservation),
        "assistant_output" => Ok(MessageRole::AssistantOutput),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown message role {value}"
        ))),
    }
}

fn parse_status(value: &str) -> Result<ThreadStatus, InboundEventStoreError> {
    match value {
        "open" => Ok(ThreadStatus::Open),
        "waiting" => Ok(ThreadStatus::Waiting),
        "resolved" => Ok(ThreadStatus::Resolved),
        "closed" => Ok(ThreadStatus::Closed),
        "reopened" => Ok(ThreadStatus::Reopened),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown thread status {value}"
        ))),
    }
}

#[derive(Debug, FromQueryResult)]
struct SemanticThreadRow {
    thread_id: String,
    status: String,
    last_added_at: Option<NaiveDateTime>,
    last_source_event_id: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct SemanticEventRow {
    source_event_id: String,
    added_at: NaiveDateTime,
    source_channel: String,
    platform_account_id: String,
    actor_platform_id: String,
    message_role: String,
    occurred_at_unix_secs: i64,
    normalized_text: String,
}

#[derive(Debug, FromQueryResult)]
struct ValueRow {
    value: String,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    value: i64,
}
