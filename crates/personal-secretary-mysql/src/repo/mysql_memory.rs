use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use tracing::{debug, info};
use uuid::Uuid;

use crate::{
    ContentTrustLevel, ConversationDerivedStateInvalidation, ConversationMemoryModeInput,
    ConversationMemoryModeReceipt, InboundEventStoreError, MemoryDeleteInput, MemoryDeleteReceipt,
    MemoryFact, MemoryFactId, MemoryFactStatus, MemoryFactView, MemorySourceExcerpt, MemoryStoreT,
    MemoryWriteReceipt, SourceAccountRef, SourceEventId, validate_memory_fact,
};

use super::mysql_inbound::store_error;

pub(crate) struct MySqlMemoryStore {
    db: DatabaseConnection,
}

impl MySqlMemoryStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MemoryStoreT for MySqlMemoryStore {
    async fn append_fact(
        &self,
        fact: &MemoryFact,
    ) -> Result<MemoryWriteReceipt, InboundEventStoreError> {
        validate_memory_fact(fact)
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        let transaction = self.db.begin().await.map_err(store_error)?;
        let account = AccountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT id FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ? FOR UPDATE",
            [fact.account.channel.as_str().into(), fact.account.account_id.clone().into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        .ok_or_else(|| InboundEventStoreError::InvalidData("memory account was not found".into()))?;

        if let Some(existing) = MemoryJsonRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(fact_json AS CHAR) AS fact_json FROM secretary_memory_facts WHERE fact_id = ? FOR UPDATE",
            [fact.fact_id.as_str().into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        {
            let existing_fact: MemoryFact = serde_json::from_str(&existing.fact_json).map_err(|error| {
                InboundEventStoreError::InvalidData(format!("stored memory fact is invalid: {error}"))
            })?;
            if existing_fact == *fact {
                transaction.commit().await.map_err(store_error)?;
                return Ok(MemoryWriteReceipt {
                    fact_id: fact.fact_id.clone(),
                    changed: false,
                });
            }
            return Err(InboundEventStoreError::InvalidData(
                "memory fact id already exists with different immutable content".into(),
            ));
        }

        if fact.supersedes_fact_id.is_none()
            && ActiveSubjectRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"SELECT fact_id FROM secretary_memory_facts
                   WHERE account_id = ? AND fact_kind = ? AND subject_key = ?
                     AND fact_status IN ('proposed', 'confirmed')
                   LIMIT 1 FOR UPDATE"#,
                [
                    account.id.into(),
                    fact.payload.kind().into(),
                    fact.subject_key.clone().into(),
                ],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?
            .is_some()
        {
            return Err(InboundEventStoreError::InvalidData(
                "an active memory fact already exists for this subject; reread its sources and provide supersedes_fact_id".into(),
            ));
        }

        for source_event_id in &fact.source_event_ids {
            let source = MemorySourceRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"SELECT event.account_id, conversation.memory_mode, content.content_mode
                   FROM secretary_source_events event
                   JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
                   JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
                   WHERE event.source_event_id = ? FOR UPDATE"#,
                [source_event_id.as_str().into()],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?
            .ok_or_else(|| InboundEventStoreError::InvalidData(format!("memory source event {} was not found", source_event_id.as_str())))?;
            if source.account_id != account.id {
                return Err(InboundEventStoreError::InvalidData(
                    "memory source event cannot cross managed accounts".into(),
                ));
            }
            if !matches!(source.memory_mode.as_str(), "normal" | "local_only")
                || !matches!(source.content_mode.as_str(), "normal" | "local_only")
            {
                return Err(InboundEventStoreError::InvalidData(
                    "conversation or content policy forbids long-term memory derivation".into(),
                ));
            }
        }

        if let Some(previous_id) = &fact.supersedes_fact_id {
            let previous = PreviousFactRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT account_id, fact_kind, subject_key, fact_status FROM secretary_memory_facts WHERE fact_id = ? FOR UPDATE",
                [previous_id.as_str().into()],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?
            .ok_or_else(|| InboundEventStoreError::InvalidData("superseded memory fact was not found".into()))?;
            if previous.account_id != account.id
                || previous.fact_kind != fact.payload.kind()
                || previous.subject_key != fact.subject_key
                || !matches!(previous.fact_status.as_str(), "proposed" | "confirmed")
            {
                return Err(InboundEventStoreError::InvalidData(
                    "superseded memory fact is outside the account or no longer active".into(),
                ));
            }
            let updated = transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "UPDATE secretary_memory_facts SET fact_status = 'superseded' WHERE fact_id = ? AND fact_status IN ('proposed', 'confirmed')",
                    [previous_id.as_str().into()],
                ))
                .await
                .map_err(store_error)?;
            if updated.rows_affected() != 1 {
                return Err(InboundEventStoreError::LeaseLost);
            }
        }

        let fact_json = serde_json::to_string(fact).map_err(|error| {
            InboundEventStoreError::InvalidData(format!("cannot serialize memory fact: {error}"))
        })?;
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_memory_facts
                   (fact_id, account_id, fact_kind, subject_key, fact_json, fact_status,
                    confidence_bps, valid_until_unix_secs, supersedes_fact_id)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                [
                    fact.fact_id.as_str().into(),
                    account.id.into(),
                    fact.payload.kind().into(),
                    fact.subject_key.clone().into(),
                    fact_json.into(),
                    fact.status.as_str().into(),
                    fact.confidence_bps.into(),
                    fact.valid_until_unix_secs.into(),
                    fact.supersedes_fact_id
                        .as_ref()
                        .map(|id| id.as_str().to_owned())
                        .into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        for source_event_id in &fact.source_event_ids {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "INSERT INTO secretary_memory_fact_sources (fact_id, source_event_id) VALUES (?, ?)",
                    [fact.fact_id.as_str().into(), source_event_id.as_str().into()],
                ))
                .await
                .map_err(store_error)?;
        }
        transaction.commit().await.map_err(store_error)?;
        info!(
            fact_id = fact.fact_id.as_str(),
            kind = fact.payload.kind(),
            sources = fact.source_event_ids.len(),
            "source-backed structured memory fact persisted"
        );
        Ok(MemoryWriteReceipt {
            fact_id: fact.fact_id.clone(),
            changed: true,
        })
    }

    async fn list_active(
        &self,
        account: &SourceAccountRef,
        limit: u32,
    ) -> Result<Vec<MemoryFact>, InboundEventStoreError> {
        if !(1..=200).contains(&limit) {
            return Err(InboundEventStoreError::InvalidData(
                "memory query limit must be in 1..=200".into(),
            ));
        }
        let now = Utc::now().timestamp();
        MemoryJsonRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT CAST(fact.fact_json AS CHAR) AS fact_json
               FROM secretary_memory_facts fact
               JOIN secretary_accounts account ON account.id = fact.account_id
               WHERE account.source_channel = ? AND account.platform_account_id = ?
                 AND fact.fact_status IN ('proposed', 'confirmed')
                 AND (fact.valid_until_unix_secs IS NULL OR fact.valid_until_unix_secs > ?)
                 AND EXISTS (SELECT 1 FROM secretary_memory_fact_sources source0
                             WHERE source0.fact_id = fact.fact_id)
                 AND NOT EXISTS (
                     SELECT 1 FROM secretary_memory_fact_sources source
                     LEFT JOIN secretary_source_events event
                       ON event.source_event_id = source.source_event_id
                     LEFT JOIN secretary_conversations conversation
                       ON conversation.id = event.conversation_id
                     LEFT JOIN secretary_message_contents content
                       ON content.source_event_id = event.source_event_id
                     LEFT JOIN secretary_message_tombstones tombstone
                       ON tombstone.source_event_id = event.source_event_id
                      AND tombstone.account_id = event.account_id
                      AND tombstone.status = 'applied'
                     WHERE source.fact_id = fact.fact_id
                       AND (event.source_event_id IS NULL
                            OR (event.message_role <> 'owner_command'
                                AND (event.account_id <> fact.account_id
                                     OR conversation.memory_mode <> 'normal'
                                     OR content.source_event_id IS NULL
                                     OR content.content_mode <> 'normal'))
                            OR tombstone.source_event_id IS NOT NULL)
                 )
               ORDER BY fact.updated_at DESC, fact.fact_id DESC LIMIT ?"#,
            [
                account.channel.as_str().into(),
                account.account_id.clone().into(),
                now.into(),
                limit.into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?
        .into_iter()
        .map(|row| {
            serde_json::from_str(&row.fact_json).map_err(|error| {
                InboundEventStoreError::InvalidData(format!(
                    "stored memory fact is invalid: {error}"
                ))
            })
        })
        .collect()
    }

    async fn expire_due(
        &self,
        now_unix_secs: i64,
        limit: u32,
    ) -> Result<u64, InboundEventStoreError> {
        if !(1..=1000).contains(&limit) {
            return Err(InboundEventStoreError::InvalidData(
                "memory expiry limit must be in 1..=1000".into(),
            ));
        }
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_memory_facts
                   SET fact_status = 'expired'
                   WHERE fact_status IN ('proposed', 'confirmed')
                     AND valid_until_unix_secs IS NOT NULL AND valid_until_unix_secs <= ?
                   ORDER BY valid_until_unix_secs, fact_id LIMIT ?"#,
                [now_unix_secs.into(), limit.into()],
            ))
            .await
            .map_err(store_error)?;
        debug!(
            expired = result.rows_affected(),
            "expired structured memory facts"
        );
        Ok(result.rows_affected())
    }

    async fn load_with_sources(
        &self,
        fact_id: &MemoryFactId,
        max_excerpt_chars: u32,
    ) -> Result<Option<MemoryFactView>, InboundEventStoreError> {
        if !(1..=2000).contains(&max_excerpt_chars) {
            return Err(InboundEventStoreError::InvalidData(
                "memory source excerpt limit must be in 1..=2000".into(),
            ));
        }
        let Some(row) = MemoryFactRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(fact_json AS CHAR) AS fact_json, fact_status FROM secretary_memory_facts WHERE fact_id = ?",
            [fact_id.as_str().into()],
        ))
        .one(&self.db)
        .await
        .map_err(store_error)? else {
            return Ok(None);
        };
        let mut fact: MemoryFact = serde_json::from_str(&row.fact_json).map_err(|error| {
            InboundEventStoreError::InvalidData(format!("stored memory fact is invalid: {error}"))
        })?;
        fact.status = parse_fact_status(&row.fact_status)?;
        let sources = MemoryExcerptRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT event.source_event_id, conversation.conversation_kind,
                      conversation.platform_conversation_id, event.actor_platform_id,
                      event.occurred_at_unix_secs, COALESCE(content.normalized_text, '') AS normalized_text,
                      CASE
                        WHEN event.message_role = 'owner_command'
                          THEN 'envelope_only'
                        WHEN conversation.memory_mode = 'local_only'
                          OR content.content_mode = 'local_only'
                        THEN 'local_only'
                        ELSE 'normal'
                      END AS content_trust_level
               FROM secretary_memory_fact_sources source
               JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
               JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
               LEFT JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
               WHERE source.fact_id = ?
                 AND (conversation.memory_mode = 'normal'
                      OR event.message_role = 'owner_command')
                 AND (event.message_role = 'owner_command'
                      OR (conversation.memory_mode = 'normal' AND content.content_mode = 'normal'))
                 -- CMD-009 目标 C：冲突回读的“来源有效”必须排除已撤回消息；
                 -- 任一关键来源失效即 fail-closed，不把旧事实呈现为有效。
                 AND NOT EXISTS (
                     SELECT 1 FROM secretary_message_tombstones t
                     WHERE t.source_event_id = event.source_event_id
                       AND t.account_id = event.account_id
                       AND t.status = 'applied'
                 )
               ORDER BY event.occurred_at_unix_secs, event.source_event_id"#,
            [fact_id.as_str().into()],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?
        .into_iter()
        .map(|source| {
            let trust = parse_content_trust_level(&source.content_trust_level)?;
            Ok(MemorySourceExcerpt {
                source_event_id: SourceEventId::new(source.source_event_id)?,
                conversation_kind: source.conversation_kind,
                conversation_id: source.platform_conversation_id,
                actor_id: source.actor_platform_id,
                occurred_at_unix_secs: source.occurred_at_unix_secs,
                excerpt: filter_memory_excerpt(
                    source.normalized_text.chars().take(max_excerpt_chars as usize).collect(),
                    trust,
                ),
                content_trust_level: trust,
            })
        })
        .collect::<Result<Vec<_>, InboundEventStoreError>>()?;
        let all_sources_visible = sources.len() == fact.source_event_ids.len()
            && fact.source_event_ids.iter().all(|expected| {
                sources
                    .iter()
                    .any(|source| &source.source_event_id == expected)
            });
        if !all_sources_visible {
            return Ok(None);
        }
        debug!(
            fact_id = fact_id.as_str(),
            sources = sources.len(),
            "loaded memory evidence"
        );
        Ok(Some(MemoryFactView { fact, sources }))
    }

    async fn delete_derived(
        &self,
        input: &MemoryDeleteInput,
    ) -> Result<MemoryDeleteReceipt, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let context = MemoryDeleteContextRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT fact.fact_status, command.message_role, command.source_event_id,
                      command.actor_platform_id
               FROM secretary_memory_facts fact
               JOIN secretary_source_events command ON command.source_event_id = ?
               JOIN secretary_owner_bindings binding
                 ON binding.managed_account_id = fact.account_id
                AND binding.command_account_id = command.account_id
                AND binding.owner_actor_id = command.actor_platform_id
                AND binding.status = 'active'
               WHERE fact.fact_id = ? AND command.actor_kind = 'owner' FOR UPDATE"#,
            [
                input.command_source_event_id.as_str().into(),
                input.fact_id.as_str().into(),
            ],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        .ok_or_else(|| {
            InboundEventStoreError::InvalidData(
                "memory fact or authorized Owner command was not found".into(),
            )
        })?;
        if context.message_role != "owner_command" {
            return Err(InboundEventStoreError::InvalidData(
                "derived memory deletion requires an OwnerCommand event".into(),
            ));
        }
        if let Some(existing) = MemoryDeletionRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT command_source_event_id, reason FROM secretary_memory_deletions WHERE fact_id = ? FOR UPDATE",
            [input.fact_id.as_str().into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        {
            if existing.command_source_event_id == input.command_source_event_id.as_str()
                && existing.reason == input.reason
            {
                transaction.commit().await.map_err(store_error)?;
                return Ok(MemoryDeleteReceipt { fact_id: input.fact_id.clone(), changed: false });
            }
            return Err(InboundEventStoreError::InvalidData(
                "memory fact already has a different immutable deletion record".into(),
            ));
        }
        if matches!(
            context.fact_status.as_str(),
            "deleted" | "expired" | "superseded"
        ) {
            return Err(InboundEventStoreError::InvalidData(
                "memory fact is no longer active and cannot be deleted".into(),
            ));
        }
        transaction.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_memory_deletions (fact_id, command_source_event_id, owner_actor_id, reason) VALUES (?, ?, ?, ?)",
            [
                input.fact_id.as_str().into(),
                input.command_source_event_id.as_str().into(),
                context.actor_platform_id.into(),
                input.reason.clone().into(),
            ],
        )).await.map_err(store_error)?;
        transaction.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_memory_facts SET fact_status = 'deleted' WHERE fact_id = ? AND fact_status IN ('proposed', 'confirmed')",
            [input.fact_id.as_str().into()],
        )).await.map_err(store_error)?;
        transaction.commit().await.map_err(store_error)?;
        info!(
            fact_id = input.fact_id.as_str(),
            command_source_event_id = input.command_source_event_id.as_str(),
            "derived memory deleted by authorized owner command"
        );
        Ok(MemoryDeleteReceipt {
            fact_id: input.fact_id.clone(),
            changed: true,
        })
    }

    async fn set_conversation_mode(
        &self,
        input: &ConversationMemoryModeInput,
    ) -> Result<ConversationMemoryModeReceipt, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let context =
            ConversationModeContextRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"SELECT managed.id AS account_row_id,
                          conversation.id AS conversation_row_id,
                          conversation.memory_mode, command.message_role
                   FROM secretary_accounts managed
                   JOIN secretary_conversations conversation
                     ON conversation.account_id = managed.id
                    AND conversation.conversation_kind = ?
                    AND conversation.platform_conversation_id = ?
                   JOIN secretary_source_events command
                     ON command.source_event_id = ?
                   JOIN secretary_owner_bindings binding
                     ON binding.managed_account_id = managed.id
                    AND binding.command_account_id = command.account_id
                    AND binding.owner_actor_id = command.actor_platform_id
                    AND binding.status = 'active'
                   WHERE managed.source_channel = ?
                     AND managed.platform_account_id = ?
                     AND command.actor_kind = 'owner'
                   FOR UPDATE"#,
                [
                    input.conversation.kind.as_str().into(),
                    input.conversation.id.clone().into(),
                    input.command_source_event_id.as_str().into(),
                    input.account.channel.as_str().into(),
                    input.account.account_id.clone().into(),
                ],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?
            .ok_or_else(|| {
                InboundEventStoreError::InvalidData(
                    "conversation or authorized Owner command was not found".into(),
                )
            })?;
        if context.message_role != "owner_command" {
            return Err(InboundEventStoreError::InvalidData(
                "conversation memory mode requires an OwnerCommand event".into(),
            ));
        }
        let previous_mode = parse_content_trust_level(&context.memory_mode)?;
        if previous_mode == input.mode {
            transaction.commit().await.map_err(store_error)?;
            return Ok(ConversationMemoryModeReceipt {
                changed: false,
                previous_mode,
                current_mode: input.mode,
                invalidated: ConversationDerivedStateInvalidation::default(),
            });
        }
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_conversations SET memory_mode = ? WHERE id = ?",
                [
                    input.mode.as_str().into(),
                    context.conversation_row_id.into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        let invalidated = reconcile_conversation_visibility_in_txn(
            &transaction,
            context.account_row_id,
            context.conversation_row_id,
            input.mode,
        )
        .await?;
        transaction.commit().await.map_err(store_error)?;
        info!(
            conversation_kind = input.conversation.kind.as_str(),
            memory_mode = input.mode.as_str(),
            invalidated = invalidated.total(),
            "conversation memory mode updated by authorized owner command"
        );
        Ok(ConversationMemoryModeReceipt {
            changed: true,
            previous_mode,
            current_mode: input.mode,
            invalidated,
        })
    }
}

async fn reconcile_conversation_visibility_in_txn(
    transaction: &sea_orm::DatabaseTransaction,
    account_id: u64,
    conversation_id: u64,
    mode: ContentTrustLevel,
) -> Result<ConversationDerivedStateInvalidation, InboundEventStoreError> {
    let mut report = ConversationDerivedStateInvalidation::default();

    report.revoked_worker_leases = execute_affected(
        transaction,
        r#"DELETE state FROM secretary_thread_semantic_state state
           WHERE EXISTS (
               SELECT 1 FROM secretary_effective_thread_events te
               JOIN secretary_source_events event ON event.source_event_id = te.source_event_id
               WHERE te.thread_id = state.thread_id AND event.conversation_id = ?
           )"#,
        [conversation_id.into()],
    )
    .await?;
    report.revoked_worker_leases = report.revoked_worker_leases.saturating_add(
        execute_affected(
            transaction,
            r#"DELETE scan FROM secretary_thread_link_scan_state scan
               JOIN secretary_source_events event ON event.source_event_id = scan.source_event_id
               WHERE event.conversation_id = ?"#,
            [conversation_id.into()],
        )
        .await?,
    );
    report.revoked_worker_leases = report.revoked_worker_leases.saturating_add(
        execute_affected(
            transaction,
            "DELETE FROM secretary_memory_candidate_processing_state WHERE account_id = ?",
            [account_id.into()],
        )
        .await?,
    );

    if mode == ContentTrustLevel::Normal {
        return Ok(report);
    }

    report.revoked_action_runs = execute_affected(
        transaction,
        r#"UPDATE secretary_action_runs run
           SET run.status = 'failed', run.lease_token = NULL, run.lease_expires_at = NULL,
               run.worker_id = NULL, run.last_error = 'source_content_restricted',
               run.updated_at = CURRENT_TIMESTAMP(6)
           WHERE run.account_id = ? AND run.status IN ('pending', 'running', 'suspended')
             AND EXISTS (
                 SELECT 1 FROM secretary_source_events event
                 WHERE event.conversation_id = ?
                   AND JSON_SEARCH(
                       run.recent_events_json, 'one', event.source_event_id,
                       NULL, '$[*].source_event_id'
                   ) IS NOT NULL
             )"#,
        [account_id.into(), conversation_id.into()],
    )
    .await?;

    report.owner_response_drafts = execute_affected(
        transaction,
        r#"UPDATE secretary_action_responses response
           JOIN secretary_action_runs run ON run.run_id = response.run_id
           SET response.invalidated = 1,
               response.response_json = JSON_SET(response.response_json, '$.invalidated', TRUE)
           WHERE run.account_id = ? AND response.invalidated = 0
             AND EXISTS (
                 SELECT 1 FROM JSON_TABLE(CAST(response.response_json AS CHAR), '$.source_event_ids[*]'
                     COLUMNS (source_event_id VARCHAR(191) PATH '$')) source_ref
                 JOIN secretary_source_events event
                   ON event.source_event_id = source_ref.source_event_id
                 WHERE event.conversation_id = ?
             )"#,
        [account_id.into(), conversation_id.into()],
    )
    .await?;
    execute_affected(
        transaction,
        r#"UPDATE secretary_action_runs run
           SET run.response_draft_json = JSON_SET(run.response_draft_json, '$.invalidated', TRUE),
               run.updated_at = CURRENT_TIMESTAMP(6)
           WHERE run.account_id = ? AND run.response_draft_json IS NOT NULL
             AND EXISTS (
                 SELECT 1 FROM JSON_TABLE(CAST(run.response_draft_json AS CHAR), '$.source_event_ids[*]'
                     COLUMNS (source_event_id VARCHAR(191) PATH '$')) source_ref
                 JOIN secretary_source_events event
                   ON event.source_event_id = source_ref.source_event_id
                 WHERE event.conversation_id = ?
             )"#,
        [account_id.into(), conversation_id.into()],
    )
    .await?;

    report.semantic_claims = execute_affected(
        transaction,
        r#"UPDATE secretary_thread_claims claim
           SET claim.status = 'withdrawn', claim.updated_at = CURRENT_TIMESTAMP(6)
           WHERE claim.status IN ('proposed', 'contested', 'confirmed')
             AND EXISTS (
                 SELECT 1 FROM secretary_thread_claim_sources source
                 JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
                 WHERE source.claim_id = claim.claim_id AND event.conversation_id = ?
             )"#,
        [conversation_id.into()],
    )
    .await?;
    report.semantic_decisions = execute_affected(
        transaction,
        r#"UPDATE secretary_thread_decisions decision_row
           SET decision_row.status = 'revoked', decision_row.updated_at = CURRENT_TIMESTAMP(6)
           WHERE decision_row.status IN ('proposed', 'confirmed')
             AND EXISTS (
                 SELECT 1 FROM secretary_thread_decision_sources source
                 JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
                 WHERE source.decision_id = decision_row.decision_id AND event.conversation_id = ?
             )"#,
        [conversation_id.into()],
    )
    .await?;
    report.open_questions = execute_affected(
        transaction,
        r#"UPDATE secretary_thread_open_questions question
           SET question.status = 'dismissed', question.updated_at = CURRENT_TIMESTAMP(6)
           WHERE question.status = 'open'
             AND EXISTS (
                 SELECT 1 FROM secretary_thread_question_sources source
                 JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
                 WHERE source.question_id = question.question_id AND event.conversation_id = ?
             )"#,
        [conversation_id.into()],
    )
    .await?;
    report.response_expectations = execute_affected(
        transaction,
        r#"UPDATE secretary_response_expectations expectation
           SET expectation.expectation_status = 'dismissed',
               expectation.source_version = expectation.source_version + 1,
               expectation.updated_at = CURRENT_TIMESTAMP(6)
           WHERE expectation.expectation_status = 'active'
             AND EXISTS (
                 SELECT 1 FROM secretary_thread_question_sources source
                 JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
                 WHERE source.question_id = expectation.source_question_id
                   AND event.conversation_id = ?
             )"#,
        [conversation_id.into()],
    )
    .await?;
    report.thread_link_candidates = execute_affected(
        transaction,
        r#"UPDATE secretary_thread_link_candidates candidate
           SET candidate.status = 'expired', candidate.updated_at = CURRENT_TIMESTAMP(6)
           WHERE candidate.status = 'proposed'
             AND EXISTS (
                 SELECT 1 FROM secretary_thread_link_candidate_sources source
                 JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
                 WHERE source.candidate_id = candidate.candidate_id AND event.conversation_id = ?
             )"#,
        [conversation_id.into()],
    )
    .await?;
    report.memory_candidates = execute_affected(
        transaction,
        r#"UPDATE secretary_memory_candidates candidate
           SET candidate.candidate_status = 'invalidated',
               candidate.candidate_version = candidate.candidate_version + 1,
               candidate.updated_at = CURRENT_TIMESTAMP(6)
           WHERE candidate.candidate_status = 'proposed'
             AND EXISTS (
                 SELECT 1 FROM secretary_memory_candidate_sources source
                 JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
                 WHERE source.candidate_id = candidate.candidate_id AND event.conversation_id = ?
             )"#,
        [conversation_id.into()],
    )
    .await?;
    report.follow_ups = execute_affected(
        transaction,
        r#"UPDATE secretary_follow_up_items item
           SET item.status = 'superseded', item.source_version = item.source_version + 1,
               item.updated_at = CURRENT_TIMESTAMP(6)
           WHERE item.status = 'scheduled'
             AND EXISTS (
                 SELECT 1 FROM secretary_memory_fact_sources source
                 JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
                 WHERE source.fact_id = item.source_memory_fact_id AND event.conversation_id = ?
             )"#,
        [conversation_id.into()],
    )
    .await?;
    report.notification_outbox = execute_affected(
        transaction,
        r#"UPDATE secretary_notification_outbox notification
           SET notification.delivery_status = 'suppressed', notification.lease_token = NULL,
               notification.lease_expires_at = NULL,
               notification.last_error_code = 'source_content_restricted'
           WHERE notification.delivery_status IN ('pending', 'failed')
             AND (
                 EXISTS (
                     SELECT 1 FROM secretary_follow_up_items item
                     JOIN secretary_memory_fact_sources source
                       ON source.fact_id = item.source_memory_fact_id
                     JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
                     WHERE item.follow_up_id = notification.follow_up_id
                       AND event.conversation_id = ?
                 )
                 OR EXISTS (
                     SELECT 1 FROM secretary_notification_candidates candidate
                     JOIN secretary_response_expectations expectation
                       ON candidate.source_kind = 'response_expectation'
                      AND candidate.source_id = expectation.expectation_id
                     JOIN secretary_thread_question_sources source
                       ON source.question_id = expectation.source_question_id
                     JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
                     WHERE candidate.notification_candidate_id = notification.notification_candidate_id
                       AND event.conversation_id = ?
                 )
             )"#,
        [conversation_id.into(), conversation_id.into()],
    )
    .await?;
    report.memory_facts = execute_affected(
        transaction,
        r#"UPDATE secretary_memory_facts fact
           SET fact.fact_status = 'expired', fact.updated_at = CURRENT_TIMESTAMP(6)
           WHERE fact.fact_status IN ('proposed', 'confirmed')
             AND EXISTS (
                 SELECT 1 FROM secretary_memory_fact_sources source
                 JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
                 WHERE source.fact_id = fact.fact_id AND event.conversation_id = ?
             )"#,
        [conversation_id.into()],
    )
    .await?;
    report.participant_observations = execute_affected(
        transaction,
        r#"UPDATE secretary_participant_conversation_observations observation
           SET observation.invalidated = 1,
               observation.invalidation_reason = 'source_content_restricted',
               observation.updated_at = CURRENT_TIMESTAMP(6)
           WHERE observation.conversation_id = ? AND observation.invalidated = 0"#,
        [conversation_id.into()],
    )
    .await?;
    report.participant_profiles = execute_affected(
        transaction,
        r#"UPDATE secretary_participant_profiles profile
           SET profile.invalidated = 1,
               profile.invalidation_reason = 'source_content_restricted',
               profile.updated_at = CURRENT_TIMESTAMP(6)
           WHERE profile.account_id = ? AND profile.invalidated = 0
             AND (
                 EXISTS (
                     SELECT 1 FROM secretary_source_events event
                     WHERE event.source_event_id = profile.established_by_event_id
                       AND event.conversation_id = ?
                 )
                 OR EXISTS (
                     SELECT 1
                     FROM JSON_TABLE(profile.source_event_ids_json, '$[*]'
                         COLUMNS (source_event_id CHAR(36) PATH '$')) source_ref
                     JOIN secretary_source_events event
                       ON event.source_event_id = source_ref.source_event_id
                     WHERE event.conversation_id = ?
                 )
             )"#,
        [
            account_id.into(),
            conversation_id.into(),
            conversation_id.into(),
        ],
    )
    .await?;
    report.reopened_threads =
        reopen_threads_with_restricted_resolution_sources(transaction, conversation_id).await?;

    Ok(report)
}

async fn reopen_threads_with_restricted_resolution_sources(
    transaction: &sea_orm::DatabaseTransaction,
    conversation_id: u64,
) -> Result<u64, InboundEventStoreError> {
    let rows = AffectedThreadRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT thread.thread_id
           FROM secretary_event_threads thread
           WHERE thread.status = 'resolved'
             AND EXISTS (
                 SELECT 1
                 FROM secretary_thread_status_history history
                 JOIN secretary_thread_status_sources source ON source.change_id = history.change_id
                 JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
                 WHERE history.thread_id = thread.thread_id
                   AND history.change_id = (
                       SELECT latest.change_id FROM secretary_thread_status_history latest
                       WHERE latest.thread_id = thread.thread_id
                       ORDER BY latest.created_at DESC, latest.change_id DESC LIMIT 1
                   )
                   AND history.to_status = 'resolved'
                   AND history.authority = 'evidence_derived'
                   AND event.conversation_id = ?
             )
           FOR UPDATE"#,
        [conversation_id.into()],
    ))
    .all(transaction)
    .await
    .map_err(store_error)?;
    let now = Utc::now().naive_utc();
    let mut reopened = 0_u64;
    for row in rows {
        let updated = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_event_threads SET status = 'reopened', updated_at = ? WHERE thread_id = ? AND status = 'resolved'",
                [now.into(), row.thread_id.clone().into()],
            ))
            .await
            .map_err(store_error)?;
        if updated.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_thread_status_history
                   (change_id, thread_id, from_status, to_status, authority, reason, created_at)
                   VALUES (?, ?, 'resolved', 'reopened', 'system_recovery',
                           'resolution source became restricted', ?)"#,
                [
                    Uuid::new_v4().to_string().into(),
                    row.thread_id.into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        reopened = reopened.saturating_add(1);
    }
    Ok(reopened)
}

async fn execute_affected<I>(
    transaction: &sea_orm::DatabaseTransaction,
    sql: &str,
    values: I,
) -> Result<u64, InboundEventStoreError>
where
    I: IntoIterator<Item = sea_orm::Value>,
{
    Ok(transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .await
        .map_err(store_error)?
        .rows_affected())
}

#[derive(Debug, FromQueryResult)]
struct AccountRow {
    id: u64,
}

#[derive(Debug, FromQueryResult)]
struct MemoryJsonRow {
    fact_json: String,
}

#[derive(Debug, FromQueryResult)]
struct MemorySourceRow {
    account_id: u64,
    memory_mode: String,
    content_mode: String,
}

#[derive(Debug, FromQueryResult)]
struct PreviousFactRow {
    account_id: u64,
    fact_kind: String,
    subject_key: String,
    fact_status: String,
}

#[derive(Debug, FromQueryResult)]
struct ActiveSubjectRow {
    #[allow(dead_code)]
    fact_id: String,
}

#[derive(Debug, FromQueryResult)]
struct MemoryFactRow {
    fact_json: String,
    fact_status: String,
}

#[derive(Debug, FromQueryResult)]
struct MemoryExcerptRow {
    source_event_id: String,
    conversation_kind: String,
    platform_conversation_id: String,
    actor_platform_id: String,
    occurred_at_unix_secs: i64,
    normalized_text: String,
    content_trust_level: String,
}

#[derive(Debug, FromQueryResult)]
struct MemoryDeleteContextRow {
    fact_status: String,
    message_role: String,
    actor_platform_id: String,
}

#[derive(Debug, FromQueryResult)]
struct MemoryDeletionRow {
    command_source_event_id: String,
    reason: String,
}

#[derive(Debug, FromQueryResult)]
struct ConversationModeContextRow {
    account_row_id: u64,
    conversation_row_id: u64,
    memory_mode: String,
    message_role: String,
}

#[derive(Debug, FromQueryResult)]
struct AffectedThreadRow {
    thread_id: String,
}

fn parse_content_trust_level(value: &str) -> Result<ContentTrustLevel, InboundEventStoreError> {
    match value {
        "normal" => Ok(ContentTrustLevel::Normal),
        "local_only" => Ok(ContentTrustLevel::LocalOnly),
        "envelope_only" => Ok(ContentTrustLevel::EnvelopeOnly),
        "never_long_term" => Ok(ContentTrustLevel::NeverLongTerm),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "stored conversation memory mode is invalid: {value}"
        ))),
    }
}

fn filter_memory_excerpt(text: String, trust: ContentTrustLevel) -> String {
    match trust {
        ContentTrustLevel::Normal | ContentTrustLevel::LocalOnly => text,
        ContentTrustLevel::EnvelopeOnly | ContentTrustLevel::NeverLongTerm => String::new(),
    }
}

fn parse_fact_status(value: &str) -> Result<MemoryFactStatus, InboundEventStoreError> {
    match value {
        "proposed" => Ok(MemoryFactStatus::Proposed),
        "confirmed" => Ok(MemoryFactStatus::Confirmed),
        "superseded" => Ok(MemoryFactStatus::Superseded),
        "expired" => Ok(MemoryFactStatus::Expired),
        "deleted" => Ok(MemoryFactStatus::Deleted),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "stored memory status is invalid: {value}"
        ))),
    }
}
