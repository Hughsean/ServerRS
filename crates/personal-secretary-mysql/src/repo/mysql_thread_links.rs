use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, FromQueryResult, QueryFilter, Set, Statement, TransactionTrait,
};
use uuid::Uuid;

use crate::{
    ClaimedThreadLinkBatch, ContentSegment, ConversationKind, ConversationRef, EventThreadId,
    InboundEventStoreError, MessageRole, MessageSource, SourceAccountRef, SourceEventId,
    ThreadActorRef, ThreadLinkCandidate, ThreadLinkCandidateCursor, ThreadLinkCandidateId,
    ThreadLinkCandidateStatus, ThreadLinkCandidateView, ThreadLinkEvent, ThreadLinkEvidence,
    ThreadLinkHint, ThreadLinkLeaseToken, ThreadLinkReviewCommand, ThreadLinkReviewContext,
    ThreadLinkReviewId, ThreadLinkReviewReceipt, ThreadLinkSourceExcerpt, ThreadLinkStoreT,
    ValidatedThreadLinkReview, validate_thread_link_candidate, validate_thread_link_review,
};

use super::entities::{secretary_thread_link_candidates, secretary_thread_link_reviews};
use super::mysql_inbound::store_error;

pub(crate) struct MySqlThreadLinkStore {
    db: DatabaseConnection,
}

impl MySqlThreadLinkStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ThreadLinkStoreT for MySqlThreadLinkStore {
    async fn claim_link_batch(
        &self,
        max_events: u32,
        max_total_chars: u32,
        lease_secs: u64,
    ) -> Result<Option<ClaimedThreadLinkBatch>, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let rows = LinkEventRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"
SELECT e.source_event_id, a.source_channel, a.platform_account_id,
       c.conversation_kind, c.platform_conversation_id, te.thread_id,
       mc.normalized_text, CAST(mc.segments AS CHAR) AS segments
FROM secretary_effective_thread_events te
JOIN secretary_source_events e ON e.source_event_id = te.source_event_id
JOIN secretary_accounts a ON a.id = e.account_id
JOIN secretary_conversations c ON c.id = e.conversation_id
JOIN secretary_message_contents mc ON mc.source_event_id = e.source_event_id
LEFT JOIN secretary_thread_link_scan_state s ON s.source_event_id = e.source_event_id
WHERE c.memory_mode IN ('normal', 'local_only')
  AND mc.content_mode IN ('normal', 'local_only')
  AND (s.source_event_id IS NULL
       OR (s.completed_at IS NULL AND (s.lease_token IS NULL OR s.lease_expires_at < ?)))
ORDER BY te.added_at ASC, e.source_event_id ASC
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

        let lease_token = ThreadLinkLeaseToken::new(Uuid::new_v4().to_string())
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        let lease_expires_at = now + Duration::seconds(lease_secs as i64);
        for row in &rows {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
INSERT INTO secretary_thread_link_scan_state
    (source_event_id, lease_token, lease_expires_at, attempts, last_error, completed_at, updated_at)
VALUES (?, ?, ?, 1, NULL, NULL, ?)
ON DUPLICATE KEY UPDATE
    lease_token = VALUES(lease_token), lease_expires_at = VALUES(lease_expires_at),
    attempts = attempts + 1, last_error = NULL, updated_at = VALUES(updated_at)
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
        transaction.commit().await.map_err(store_error)?;

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
            let segments = if content_omitted {
                Vec::new()
            } else {
                serde_json::from_str::<Vec<ContentSegment>>(&row.segments).map_err(|error| {
                    InboundEventStoreError::InvalidData(format!(
                        "invalid stored message segments for {}: {error}",
                        row.source_event_id
                    ))
                })?
            };
            events.push(ThreadLinkEvent {
                source_event_id: SourceEventId::new(row.source_event_id)?,
                account: SourceAccountRef::new(
                    parse_source(&row.source_channel)?,
                    row.platform_account_id,
                )
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                conversation: ConversationRef::new(
                    parse_conversation_kind(&row.conversation_kind)?,
                    row.platform_conversation_id,
                )
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                thread_id: EventThreadId::new(row.thread_id)
                    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                normalized_text,
                segments,
                content_omitted,
            });
        }
        Ok(Some(ClaimedThreadLinkBatch {
            lease_token,
            events,
        }))
    }

    async fn commit_link_hints(
        &self,
        lease_token: &ThreadLinkLeaseToken,
        hints: &[ThreadLinkHint],
    ) -> Result<usize, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let claimed = CountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT COUNT(*) AS value FROM secretary_thread_link_scan_state \
             WHERE lease_token = ? AND completed_at IS NULL AND lease_expires_at >= ?",
            [lease_token.as_str().into(), now.into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        .map(|row| row.value)
        .unwrap_or_default();
        if claimed == 0 {
            return Err(InboundEventStoreError::InvalidData(
                "thread link lease is missing or expired".into(),
            ));
        }

        for hint in hints {
            if !hint.kind.is_strong() || hint.fingerprint_sha256.len() != 64 {
                return Err(InboundEventStoreError::InvalidData(
                    "thread link store received invalid or weak hint".into(),
                ));
            }
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
INSERT IGNORE INTO secretary_thread_link_hints
    (hint_id, account_id, conversation_id, thread_id, source_event_id,
     signal_kind, fingerprint_sha256, created_at)
SELECT ?, e.account_id, e.conversation_id, ?, ?, ?, ?, ?
FROM secretary_source_events e
JOIN secretary_thread_link_scan_state s ON s.source_event_id = e.source_event_id
WHERE e.source_event_id = ? AND s.lease_token = ? AND s.completed_at IS NULL
"#,
                    [
                        Uuid::new_v4().to_string().into(),
                        hint.thread_id.as_str().into(),
                        hint.source_event_id.as_str().into(),
                        hint.kind.as_str().into(),
                        hint.fingerprint_sha256.clone().into(),
                        now.into(),
                        hint.source_event_id.as_str().into(),
                        lease_token.as_str().into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
        }

        let mut candidates_created = 0usize;
        for hint in hints {
            let matches = HintMatchRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"
SELECT cur.account_id, a.source_channel, a.platform_account_id,
       cur.thread_id AS current_thread_id, cur.conversation_id AS current_conversation_id,
       cc.conversation_kind AS current_conversation_kind,
       cc.platform_conversation_id AS current_platform_conversation_id,
       cur.source_event_id AS current_source_event_id,
       other.thread_id AS other_thread_id, other.conversation_id AS other_conversation_id,
       other.signal_kind AS other_signal_kind,
       oc.conversation_kind AS other_conversation_kind,
       oc.platform_conversation_id AS other_platform_conversation_id,
       other.source_event_id AS other_source_event_id
FROM secretary_thread_link_hints cur
JOIN secretary_accounts a ON a.id = cur.account_id
JOIN secretary_conversations cc ON cc.id = cur.conversation_id
JOIN secretary_thread_link_hints other
  ON other.account_id = cur.account_id
 AND other.fingerprint_sha256 = cur.fingerprint_sha256
 AND other.thread_id <> cur.thread_id
 AND other.conversation_id <> cur.conversation_id
 AND (
      (cur.signal_kind = 'explicit_file_version'
       AND other.signal_kind = 'exact_file_source_key')
      OR (cur.signal_kind = 'exact_file_source_key'
          AND other.signal_kind IN ('exact_file_source_key', 'explicit_file_version'))
      OR (cur.signal_kind NOT IN ('exact_file_source_key', 'explicit_file_version')
          AND other.signal_kind = cur.signal_kind)
 )
JOIN secretary_conversations oc ON oc.id = other.conversation_id
WHERE cur.source_event_id = ? AND cur.signal_kind = ? AND cur.fingerprint_sha256 = ?
ORDER BY other.thread_id, other.source_event_id
"#,
                [
                    hint.source_event_id.as_str().into(),
                    hint.kind.as_str().into(),
                    hint.fingerprint_sha256.clone().into(),
                ],
            ))
            .all(&transaction)
            .await
            .map_err(store_error)?;
            for matched in matches {
                let candidate_kind = if hint.kind
                    == crate::ThreadLinkSignalKind::ExplicitFileVersion
                    || matched.other_signal_kind == "explicit_file_version"
                {
                    crate::ThreadLinkSignalKind::ExplicitFileVersion
                } else {
                    hint.kind
                };
                let (
                    left_thread,
                    right_thread,
                    left_conversation,
                    right_conversation,
                    left_event,
                    right_event,
                    left_conversation_id,
                    right_conversation_id,
                ) = if matched.current_thread_id < matched.other_thread_id {
                    (
                        matched.current_thread_id.clone(),
                        matched.other_thread_id.clone(),
                        conversation(
                            &matched.current_conversation_kind,
                            &matched.current_platform_conversation_id,
                        )?,
                        conversation(
                            &matched.other_conversation_kind,
                            &matched.other_platform_conversation_id,
                        )?,
                        matched.current_source_event_id.clone(),
                        matched.other_source_event_id.clone(),
                        matched.current_conversation_id,
                        matched.other_conversation_id,
                    )
                } else {
                    (
                        matched.other_thread_id.clone(),
                        matched.current_thread_id.clone(),
                        conversation(
                            &matched.other_conversation_kind,
                            &matched.other_platform_conversation_id,
                        )?,
                        conversation(
                            &matched.current_conversation_kind,
                            &matched.current_platform_conversation_id,
                        )?,
                        matched.other_source_event_id.clone(),
                        matched.current_source_event_id.clone(),
                        matched.other_conversation_id,
                        matched.current_conversation_id,
                    )
                };
                let candidate = ThreadLinkCandidate {
                    candidate_id: ThreadLinkCandidateId::generate(),
                    account: SourceAccountRef::new(
                        parse_source(&matched.source_channel)?,
                        matched.platform_account_id,
                    )
                    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                    left_thread_id: EventThreadId::new(left_thread.clone())
                        .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                    right_thread_id: EventThreadId::new(right_thread.clone())
                        .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                    left_conversation,
                    right_conversation,
                    status: ThreadLinkCandidateStatus::Proposed,
                    confidence_bps: candidate_kind.confidence_bps(),
                    reason_code: candidate_kind.as_str().into(),
                    evidence: ThreadLinkEvidence {
                        kind: candidate_kind,
                        fingerprint_sha256: hint.fingerprint_sha256.clone(),
                        left_source_event_id: SourceEventId::new(left_event.clone())?,
                        right_source_event_id: SourceEventId::new(right_event.clone())?,
                    },
                };
                validate_thread_link_candidate(&candidate)
                    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
                let result = transaction
                    .execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::MySql,
                        r#"
INSERT IGNORE INTO secretary_thread_link_candidates
    (candidate_id, account_id, left_thread_id, right_thread_id,
     left_conversation_id, right_conversation_id, signal_kind, fingerprint_sha256,
     status, confidence_bps, reason_code, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'proposed', ?, ?, ?, ?)
"#,
                        [
                            candidate.candidate_id.as_str().into(),
                            matched.account_id.into(),
                            left_thread.into(),
                            right_thread.into(),
                            left_conversation_id.into(),
                            right_conversation_id.into(),
                            candidate_kind.as_str().into(),
                            hint.fingerprint_sha256.clone().into(),
                            candidate_kind.confidence_bps().into(),
                            candidate_kind.as_str().into(),
                            now.into(),
                            now.into(),
                        ],
                    ))
                    .await
                    .map_err(store_error)?;
                candidates_created += result.rows_affected() as usize;
                let candidate_id = CandidateIdRow::find_by_statement(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "SELECT candidate_id FROM secretary_thread_link_candidates WHERE account_id = ? \
                     AND left_thread_id = ? AND right_thread_id = ? AND signal_kind = ? \
                     AND fingerprint_sha256 = ?",
                    [
                        matched.account_id.into(),
                        candidate.left_thread_id.as_str().into(),
                        candidate.right_thread_id.as_str().into(),
                        candidate_kind.as_str().into(),
                        hint.fingerprint_sha256.clone().into(),
                    ],
                ))
                .one(&transaction)
                .await
                .map_err(store_error)?
                .ok_or_else(|| InboundEventStoreError::InvalidData("persisted link candidate was not found".into()))?;
                for source_event_id in [left_event, right_event] {
                    transaction
                        .execute_raw(Statement::from_sql_and_values(
                            DatabaseBackend::MySql,
                            "INSERT IGNORE INTO secretary_thread_link_candidate_sources \
                         (candidate_id, source_event_id) VALUES (?, ?)",
                            [
                                candidate_id.candidate_id.clone().into(),
                                source_event_id.into(),
                            ],
                        ))
                        .await
                        .map_err(store_error)?;
                }
            }
        }

        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_thread_link_scan_state SET completed_at = ?, lease_token = NULL, \
                 lease_expires_at = NULL, last_error = NULL, updated_at = ? \
                 WHERE lease_token = ? AND completed_at IS NULL",
                [now.into(), now.into(), lease_token.as_str().into()],
            ))
            .await
            .map_err(store_error)?;
        transaction.commit().await.map_err(store_error)?;
        Ok(candidates_created)
    }

    async fn fail_link_batch(
        &self,
        lease_token: &ThreadLinkLeaseToken,
        error: &str,
    ) -> Result<(), InboundEventStoreError> {
        let safe_error: String = error.chars().take(512).collect();
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_thread_link_scan_state SET lease_token = NULL, \
                 lease_expires_at = NULL, last_error = ?, updated_at = ? \
                 WHERE lease_token = ? AND completed_at IS NULL",
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

    async fn list_link_candidates(
        &self,
        account: &SourceAccountRef,
        cursor: Option<&ThreadLinkCandidateCursor>,
        limit: u32,
    ) -> Result<Vec<ThreadLinkCandidateView>, InboundEventStoreError> {
        let (sql, values) = if let Some(cursor) = cursor {
            let created_at = DateTime::from_timestamp_micros(cursor.created_at_unix_micros)
                .ok_or_else(|| {
                    InboundEventStoreError::InvalidData(
                        "thread link candidate cursor timestamp is out of range".into(),
                    )
                })?
                .naive_utc();
            (
                candidate_view_sql(true),
                vec![
                    account.channel.as_str().into(),
                    account.account_id.clone().into(),
                    created_at.into(),
                    created_at.into(),
                    cursor.candidate_id.as_str().into(),
                    limit.into(),
                ],
            )
        } else {
            (
                candidate_view_sql(false),
                vec![
                    account.channel.as_str().into(),
                    account.account_id.clone().into(),
                    limit.into(),
                ],
            )
        };
        let rows = CandidateViewRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        let mut views = Vec::with_capacity(rows.len());
        for row in rows {
            let sources = self.load_candidate_sources(&row.candidate_id).await?;
            let candidate_id = ThreadLinkCandidateId::new(row.candidate_id.clone())
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
            views.push(ThreadLinkCandidateView {
                candidate_id: candidate_id.clone(),
                left_thread_id: EventThreadId::new(row.left_thread_id)
                    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                right_thread_id: EventThreadId::new(row.right_thread_id)
                    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                left_conversation: conversation(
                    &row.left_conversation_kind,
                    &row.left_platform_conversation_id,
                )?,
                right_conversation: conversation(
                    &row.right_conversation_kind,
                    &row.right_platform_conversation_id,
                )?,
                status: parse_candidate_status(&row.status)?,
                confidence_bps: row.confidence_bps,
                reason_code: row.reason_code,
                sources,
                cursor: ThreadLinkCandidateCursor {
                    created_at_unix_micros: row.created_at.and_utc().timestamp_micros(),
                    candidate_id,
                },
            });
        }
        Ok(views)
    }

    async fn load_link_review_context(
        &self,
        candidate_id: &ThreadLinkCandidateId,
        command_source_event_id: &SourceEventId,
    ) -> Result<ThreadLinkReviewContext, InboundEventStoreError> {
        let row =
            load_review_context_row(&self.db, candidate_id, command_source_event_id, false).await?;
        review_context(candidate_id, row)
    }

    async fn commit_link_review(
        &self,
        review: &ValidatedThreadLinkReview,
    ) -> Result<ThreadLinkReviewReceipt, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let row = load_review_context_row(
            &transaction,
            &review.candidate_id,
            &review.command_source_event_id,
            true,
        )
        .await?;
        let context = review_context(&review.candidate_id, row)?;
        let revalidated = validate_thread_link_review(&context, review.action)
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        if revalidated.target_status != review.target_status
            || revalidated.owner != review.owner
            || revalidated.command_source_event_id != review.command_source_event_id
        {
            return Err(InboundEventStoreError::InvalidData(
                "thread link review changed between validation and commit".into(),
            ));
        }

        if let Some(existing) = secretary_thread_link_reviews::Entity::find()
            .filter(
                secretary_thread_link_reviews::Column::CandidateId.eq(review.candidate_id.as_str()),
            )
            .one(&transaction)
            .await
            .map_err(store_error)?
        {
            if existing.review_action == review.action.as_str()
                && existing.command_source_event_id == review.command_source_event_id.as_str()
                && context.candidate_status == review.target_status
            {
                transaction.commit().await.map_err(store_error)?;
                return Ok(ThreadLinkReviewReceipt {
                    review_id: ThreadLinkReviewId::new(existing.review_id)
                        .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                    candidate_id: review.candidate_id.clone(),
                    status: review.target_status,
                    changed: false,
                });
            }
            return Err(InboundEventStoreError::InvalidData(
                "thread link candidate already has a different final review".into(),
            ));
        }
        if context.candidate_status != ThreadLinkCandidateStatus::Proposed {
            return Err(InboundEventStoreError::InvalidData(format!(
                "thread link candidate is already {}",
                context.candidate_status.as_str()
            )));
        }

        let now = Utc::now().naive_utc();
        let result = secretary_thread_link_candidates::Entity::update_many()
            .col_expr(
                secretary_thread_link_candidates::Column::Status,
                review.target_status.as_str().into(),
            )
            .col_expr(
                secretary_thread_link_candidates::Column::UpdatedAt,
                now.into(),
            )
            .filter(
                secretary_thread_link_candidates::Column::CandidateId
                    .eq(review.candidate_id.as_str()),
            )
            .filter(secretary_thread_link_candidates::Column::Status.eq("proposed"))
            .exec(&transaction)
            .await
            .map_err(store_error)?;
        if result.rows_affected != 1 {
            return Err(InboundEventStoreError::InvalidData(
                "thread link candidate review lost its compare-and-set".into(),
            ));
        }
        secretary_thread_link_reviews::ActiveModel {
            review_id: Set(review.review_id.as_str().into()),
            candidate_id: Set(review.candidate_id.as_str().into()),
            review_action: Set(review.action.as_str().into()),
            owner_channel: Set(review.owner.account.channel.as_str().into()),
            owner_account: Set(review.owner.account.account_id.clone()),
            owner_actor_id: Set(review.owner.actor_id.clone()),
            command_source_event_id: Set(review.command_source_event_id.as_str().into()),
            created_at: Set(now),
        }
        .insert(&transaction)
        .await
        .map_err(store_error)?;
        transaction.commit().await.map_err(store_error)?;
        Ok(ThreadLinkReviewReceipt {
            review_id: review.review_id.clone(),
            candidate_id: review.candidate_id.clone(),
            status: review.target_status,
            changed: true,
        })
    }
}

impl MySqlThreadLinkStore {
    async fn load_candidate_sources(
        &self,
        candidate_id: &str,
    ) -> Result<Vec<ThreadLinkSourceExcerpt>, InboundEventStoreError> {
        CandidateSourceRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"
SELECT e.source_event_id, c.conversation_kind, c.platform_conversation_id,
       e.actor_platform_id, e.occurred_at_unix_secs, mc.normalized_text
FROM secretary_thread_link_candidate_sources source
JOIN secretary_source_events e ON e.source_event_id = source.source_event_id
JOIN secretary_conversations c ON c.id = e.conversation_id
JOIN secretary_message_contents mc ON mc.source_event_id = e.source_event_id
WHERE source.candidate_id = ?
  AND c.memory_mode IN ('normal', 'local_only')
  AND mc.content_mode IN ('normal', 'local_only')
ORDER BY e.occurred_at_unix_secs, e.source_event_id
"#,
            [candidate_id.into()],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?
        .into_iter()
        .map(|source| {
            Ok(ThreadLinkSourceExcerpt {
                source_event_id: SourceEventId::new(source.source_event_id)?,
                conversation: conversation(
                    &source.conversation_kind,
                    &source.platform_conversation_id,
                )?,
                actor_id: source.actor_platform_id,
                occurred_at_unix_secs: source.occurred_at_unix_secs,
                excerpt: source.normalized_text.chars().take(500).collect(),
            })
        })
        .collect()
    }
}

fn candidate_view_sql(with_cursor: bool) -> &'static str {
    if with_cursor {
        r#"
SELECT candidate.candidate_id, candidate.left_thread_id, candidate.right_thread_id,
       lc.conversation_kind AS left_conversation_kind,
       lc.platform_conversation_id AS left_platform_conversation_id,
       rc.conversation_kind AS right_conversation_kind,
       rc.platform_conversation_id AS right_platform_conversation_id,
       candidate.status, candidate.confidence_bps, candidate.reason_code, candidate.created_at
FROM secretary_thread_link_candidates candidate
JOIN secretary_accounts account ON account.id = candidate.account_id
JOIN secretary_conversations lc ON lc.id = candidate.left_conversation_id
JOIN secretary_conversations rc ON rc.id = candidate.right_conversation_id
WHERE account.source_channel = ? AND account.platform_account_id = ?
  AND (candidate.created_at > ?
       OR (candidate.created_at = ? AND candidate.candidate_id > ?))
ORDER BY candidate.created_at ASC, candidate.candidate_id ASC
LIMIT ?
"#
    } else {
        r#"
SELECT candidate.candidate_id, candidate.left_thread_id, candidate.right_thread_id,
       lc.conversation_kind AS left_conversation_kind,
       lc.platform_conversation_id AS left_platform_conversation_id,
       rc.conversation_kind AS right_conversation_kind,
       rc.platform_conversation_id AS right_platform_conversation_id,
       candidate.status, candidate.confidence_bps, candidate.reason_code, candidate.created_at
FROM secretary_thread_link_candidates candidate
JOIN secretary_accounts account ON account.id = candidate.account_id
JOIN secretary_conversations lc ON lc.id = candidate.left_conversation_id
JOIN secretary_conversations rc ON rc.id = candidate.right_conversation_id
WHERE account.source_channel = ? AND account.platform_account_id = ?
ORDER BY candidate.created_at ASC, candidate.candidate_id ASC
LIMIT ?
"#
    }
}

async fn load_review_context_row<C: ConnectionTrait>(
    connection: &C,
    candidate_id: &ThreadLinkCandidateId,
    command_source_event_id: &SourceEventId,
    for_update: bool,
) -> Result<ReviewContextRow, InboundEventStoreError> {
    let suffix = if for_update { " FOR UPDATE" } else { "" };
    let sql = format!(
        r#"
SELECT candidate.status AS candidate_status,
       candidate_account.source_channel AS candidate_source_channel,
       candidate_account.platform_account_id AS candidate_platform_account_id,
       command.source_event_id AS command_source_event_id,
       command.message_role AS command_role, command.actor_platform_id AS command_actor_id,
       command_account.source_channel AS command_source_channel,
       command_account.platform_account_id AS command_platform_account_id
FROM secretary_thread_link_candidates candidate
JOIN secretary_accounts candidate_account ON candidate_account.id = candidate.account_id
JOIN secretary_source_events command ON command.source_event_id = ?
JOIN secretary_accounts command_account ON command_account.id = command.account_id
JOIN secretary_owner_bindings binding
  ON binding.managed_account_id = candidate.account_id
 AND binding.command_account_id = command.account_id
 AND binding.owner_actor_id = command.actor_platform_id
 AND binding.status = 'active'
WHERE candidate.candidate_id = ?{suffix}
"#
    );
    ReviewContextRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        sql,
        [
            command_source_event_id.as_str().into(),
            candidate_id.as_str().into(),
        ],
    ))
    .one(connection)
    .await
    .map_err(store_error)?
    .ok_or_else(|| {
        InboundEventStoreError::InvalidData(
            "thread link candidate or review command event was not found".into(),
        )
    })
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

fn parse_conversation_kind(value: &str) -> Result<ConversationKind, InboundEventStoreError> {
    match value {
        "private" => Ok(ConversationKind::Private),
        "group" => Ok(ConversationKind::Group),
        "owner_control" => Ok(ConversationKind::OwnerControl),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown conversation kind {value}"
        ))),
    }
}

fn conversation(kind: &str, id: &str) -> Result<ConversationRef, InboundEventStoreError> {
    ConversationRef::new(parse_conversation_kind(kind)?, id)
        .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))
}

fn parse_candidate_status(
    value: &str,
) -> Result<ThreadLinkCandidateStatus, InboundEventStoreError> {
    match value {
        "proposed" => Ok(ThreadLinkCandidateStatus::Proposed),
        "accepted" => Ok(ThreadLinkCandidateStatus::Accepted),
        "rejected" => Ok(ThreadLinkCandidateStatus::Rejected),
        "expired" => Ok(ThreadLinkCandidateStatus::Expired),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown thread link candidate status {value}"
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

fn review_context(
    candidate_id: &ThreadLinkCandidateId,
    row: ReviewContextRow,
) -> Result<ThreadLinkReviewContext, InboundEventStoreError> {
    let candidate_account = SourceAccountRef::new(
        parse_source(&row.candidate_source_channel)?,
        row.candidate_platform_account_id,
    )
    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
    let command_account = SourceAccountRef::new(
        parse_source(&row.command_source_channel)?,
        row.command_platform_account_id,
    )
    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
    Ok(ThreadLinkReviewContext {
        candidate_id: candidate_id.clone(),
        candidate_account: candidate_account.clone(),
        candidate_status: parse_candidate_status(&row.candidate_status)?,
        command: ThreadLinkReviewCommand {
            source_event_id: SourceEventId::new(row.command_source_event_id)?,
            actor: ThreadActorRef {
                account: command_account,
                actor_id: row.command_actor_id,
                platform_identity_kind: None,
            },
            role: parse_role(&row.command_role)?,
            authorized_account: candidate_account,
        },
    })
}

#[derive(Debug, FromQueryResult)]
struct LinkEventRow {
    source_event_id: String,
    source_channel: String,
    platform_account_id: String,
    conversation_kind: String,
    platform_conversation_id: String,
    thread_id: String,
    normalized_text: String,
    segments: String,
}

#[derive(Debug, FromQueryResult)]
struct HintMatchRow {
    account_id: u64,
    source_channel: String,
    platform_account_id: String,
    current_thread_id: String,
    current_conversation_id: u64,
    current_conversation_kind: String,
    current_platform_conversation_id: String,
    current_source_event_id: String,
    other_thread_id: String,
    other_conversation_id: u64,
    other_signal_kind: String,
    other_conversation_kind: String,
    other_platform_conversation_id: String,
    other_source_event_id: String,
}

#[derive(Debug, FromQueryResult)]
struct CandidateIdRow {
    candidate_id: String,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    value: i64,
}

#[derive(Debug, FromQueryResult)]
struct CandidateViewRow {
    candidate_id: String,
    left_thread_id: String,
    right_thread_id: String,
    left_conversation_kind: String,
    left_platform_conversation_id: String,
    right_conversation_kind: String,
    right_platform_conversation_id: String,
    status: String,
    confidence_bps: u16,
    reason_code: String,
    created_at: chrono::NaiveDateTime,
}

#[derive(Debug, FromQueryResult)]
struct CandidateSourceRow {
    source_event_id: String,
    conversation_kind: String,
    platform_conversation_id: String,
    actor_platform_id: String,
    occurred_at_unix_secs: i64,
    normalized_text: String,
}

#[derive(Debug, FromQueryResult)]
struct ReviewContextRow {
    candidate_status: String,
    candidate_source_channel: String,
    candidate_platform_account_id: String,
    command_source_event_id: String,
    command_role: String,
    command_actor_id: String,
    command_source_channel: String,
    command_platform_account_id: String,
}
