use std::collections::HashSet;

use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use tracing::{debug, info};

use crate::{
    AgendaItemKind, ClaimedOwnerNotification, FollowUpScanReport, FollowUpStoreT,
    InboundEventStoreError, LegacyNotificationReconciliationConfig, MemoryFact, MemoryPayload,
    MessageSource, NotificationFailureKind, NotificationId, NotificationLeaseToken,
    OwnerNotificationContent, SourceAccountRef,
};

use super::mysql_inbound::store_error;
use super::mysql_notification_candidate_producer::{
    LockedNotificationSource, produce_from_locked_source,
};

pub(crate) struct MySqlFollowUpStore {
    db: DatabaseConnection,
}

impl MySqlFollowUpStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl FollowUpStoreT for MySqlFollowUpStore {
    async fn scan_commitments(
        &self,
        now_unix_secs: i64,
        horizon_secs: i64,
        limit: u32,
    ) -> Result<FollowUpScanReport, InboundEventStoreError> {
        if !(60..=31_536_000).contains(&horizon_secs) || !(1..=1000).contains(&limit) {
            return Err(InboundEventStoreError::InvalidData(
                "follow-up scan bounds are invalid".into(),
            ));
        }
        let horizon = now_unix_secs.saturating_add(horizon_secs);
        let transaction = self.db.begin().await.map_err(store_error)?;

        // 先使失去当前来源的条目失效，避免同轮扫描把已被新事实替代的承诺误记为完成。
        let superseded = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_follow_up_items
               SET status = 'superseded', source_version = source_version + 1,
                   updated_at = CURRENT_TIMESTAMP(6)
               WHERE follow_up_id IN (
                 SELECT follow_up_id FROM (
                   SELECT item.follow_up_id
                   FROM secretary_follow_up_items item
                   JOIN secretary_memory_facts fact ON fact.fact_id = item.source_memory_fact_id
                   WHERE item.status = 'scheduled'
                     AND (fact.fact_status <> 'confirmed'
                          OR JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.status')) = 'cancelled'
                          OR EXISTS (
                              SELECT 1 FROM secretary_memory_facts successor
                              WHERE successor.supersedes_fact_id = fact.fact_id
                          ))
                   ORDER BY item.updated_at, item.follow_up_id LIMIT ?
                 ) bounded
               )"#,
                [limit.into()],
            ))
            .await
            .map_err(store_error)?
            .rows_affected();

        let completed = transaction.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"UPDATE secretary_follow_up_items
               SET status = 'completed', source_version = source_version + 1,
                   updated_at = CURRENT_TIMESTAMP(6)
               WHERE follow_up_id IN (
                 SELECT follow_up_id FROM (
                   SELECT item.follow_up_id
                   FROM secretary_follow_up_items item
                   JOIN secretary_memory_facts fact ON fact.fact_id = item.source_memory_fact_id
                   WHERE item.status = 'scheduled'
                     AND fact.fact_status = 'confirmed'
                     AND NOT EXISTS (
                         SELECT 1 FROM secretary_memory_facts successor
                         WHERE successor.supersedes_fact_id = fact.fact_id
                     )
                     AND JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.status')) = 'fulfilled'
                   ORDER BY item.updated_at, item.follow_up_id LIMIT ?
                 ) bounded
               )"#,
            [limit.into()],
        )).await.map_err(store_error)?.rows_affected();

        transaction
            .execute_raw(Statement::from_string(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_notification_outbox notification
                   JOIN secretary_follow_up_items item ON item.follow_up_id = notification.follow_up_id
                   SET notification.delivery_status = 'suppressed',
                       notification.lease_token = NULL, notification.lease_expires_at = NULL
                   WHERE notification.delivery_status IN ('pending', 'failed')
                     AND item.status <> 'scheduled'"#,
            ))
            .await
            .map_err(store_error)?;

        let materialized = transaction.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"INSERT IGNORE INTO secretary_follow_up_items
                 (follow_up_id, account_id, source_memory_fact_id, source_version, reason_code, due_at_unix_secs, status)
               SELECT UUID(), fact.account_id, fact.fact_id, 1, 'commitment_due',
                      CAST(JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.due_at_unix_secs')) AS SIGNED),
                      'scheduled'
               FROM secretary_memory_facts fact
               WHERE fact.fact_kind = 'commitment' AND fact.fact_status = 'confirmed'
                 AND NOT EXISTS (
                     SELECT 1 FROM secretary_memory_facts successor
                     WHERE successor.supersedes_fact_id = fact.fact_id
                 )
                 AND JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.status')) IN ('pending', 'proposed')
                 AND JSON_TYPE(JSON_EXTRACT(fact.fact_json, '$.payload.data.due_at_unix_secs')) = 'INTEGER'
                 AND CAST(JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.due_at_unix_secs')) AS SIGNED) <= ?
               ORDER BY fact.updated_at, fact.fact_id LIMIT ?"#,
            [horizon.into(), limit.into()],
        )).await.map_err(store_error)?.rows_affected();

        let candidate_rows = DueFollowUpItemRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT item.follow_up_id, item.account_id, item.source_version,
                          account.source_channel, account.platform_account_id
                   FROM secretary_follow_up_items item
                   JOIN secretary_memory_facts fact ON fact.fact_id = item.source_memory_fact_id
                   JOIN secretary_accounts account ON account.id = item.account_id
                   WHERE item.status = 'scheduled'
                     AND item.due_at_unix_secs <= ?
                     AND fact.fact_kind = 'commitment'
                     AND fact.fact_status = 'confirmed'
                     AND NOT EXISTS (
                         SELECT 1 FROM secretary_memory_facts successor
                         WHERE successor.supersedes_fact_id = fact.fact_id
                     )
                     AND JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.status'))
                         IN ('pending', 'proposed')
                   ORDER BY item.due_at_unix_secs, item.follow_up_id
                   LIMIT ? FOR UPDATE SKIP LOCKED"#,
            [now_unix_secs.into(), limit.into()],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?;
        let mut candidates_created = 0;
        let mut requests_created = 0;
        for item in candidate_rows {
            let production = produce_from_locked_source(
                &transaction,
                &LockedNotificationSource::FollowUp {
                    account_id: item.account_id,
                    follow_up_id: item.follow_up_id,
                    source_version: item.source_version,
                    source_channel: item.source_channel,
                    platform_account_id: item.platform_account_id,
                },
            )
            .await?;
            candidates_created += u64::from(production.candidate_created);
            requests_created += u64::from(production.request_created);
        }

        transaction.commit().await.map_err(store_error)?;
        let report = FollowUpScanReport {
            commitments_materialized: materialized,
            items_reconciled: completed.saturating_add(superseded),
            notification_candidates_created: candidates_created,
            notification_evaluation_requests_created: requests_created,
            memories_expired: 0,
            response_expectations_materialized: 0,
            response_expectations_resolved: 0,
            project_blockers_materialized: 0,
        };
        if materialized > 0
            || completed > 0
            || superseded > 0
            || candidates_created > 0
            || requests_created > 0
        {
            info!(
                commitments_materialized = report.commitments_materialized,
                items_reconciled = report.items_reconciled,
                notification_candidates_created = report.notification_candidates_created,
                notification_evaluation_requests_created =
                    report.notification_evaluation_requests_created,
                "follow-up scheduler persisted a bounded scan"
            );
        } else {
            debug!("follow-up scheduler scan had no changes");
        }
        Ok(report)
    }

    async fn scan_response_expectations(
        &self,
        now_unix_secs: i64,
        horizon_secs: i64,
        response_timeout_secs: i64,
        limit: u32,
    ) -> Result<FollowUpScanReport, InboundEventStoreError> {
        if !(60..=31_536_000).contains(&horizon_secs)
            || !(300..=2_592_000).contains(&response_timeout_secs)
            || !(1..=1000).contains(&limit)
        {
            return Err(InboundEventStoreError::InvalidData(
                "response expectation scan bounds are invalid".into(),
            ));
        }
        let horizon = now_unix_secs.saturating_add(horizon_secs);
        let transaction = self.db.begin().await.map_err(store_error)?;

        let resolved = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_response_expectations
                   SET expectation_status = 'resolved',
                       source_version = source_version + 1,
                       updated_at = CURRENT_TIMESTAMP(6)
                   WHERE expectation_id IN (
                     SELECT expectation_id FROM (
                       SELECT expectation.expectation_id
                       FROM secretary_response_expectations expectation
                       JOIN secretary_thread_open_questions question
                         ON question.question_id = expectation.source_question_id
                       JOIN secretary_event_threads thread
                         ON thread.thread_id = expectation.thread_id
                       WHERE expectation.expectation_status = 'active'
                         AND (
                             question.status <> 'open'
                             OR thread.status IN ('resolved', 'closed')
                             OR EXISTS (
                                 SELECT 1
                                 FROM secretary_thread_question_sources question_source
                                 JOIN secretary_source_events asked
                                   ON asked.source_event_id = question_source.source_event_id
                                 JOIN secretary_source_events reply
                                   ON reply.account_id = asked.account_id
                                  AND reply.conversation_id = asked.conversation_id
                                 JOIN secretary_conversations conversation
                                   ON conversation.id = asked.conversation_id
                                 LEFT JOIN secretary_thread_events reply_member
                                   ON reply_member.source_event_id = reply.source_event_id
                                 WHERE question_source.question_id = expectation.source_question_id
                                   AND (reply.occurred_at_unix_secs > asked.occurred_at_unix_secs
                                        OR (reply.occurred_at_unix_secs = asked.occurred_at_unix_secs
                                            AND reply.source_event_id > asked.source_event_id))
                                   AND (
                                       (reply.message_role = 'assistant_output'
                                        AND reply_member.thread_id = expectation.thread_id)
                                       OR
                                       (reply.message_role = 'owner_observation'
                                        AND (
                                            conversation.conversation_kind = 'private'
                                            OR reply_member.thread_id = expectation.thread_id
                                            OR reply.reply_to_event_id = asked.source_event_id
                                        ))
                                   )
                             )
                         )
                       ORDER BY expectation.updated_at, expectation.expectation_id
                       LIMIT ?
                     ) bounded
                   )"#,
                [limit.into()],
            ))
            .await
            .map_err(store_error)?
            .rows_affected();

        transaction
            .execute_raw(Statement::from_string(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_notification_outbox notification
                   JOIN secretary_notification_candidates candidate
                     ON candidate.notification_candidate_id = notification.notification_candidate_id
                    AND candidate.source_kind = 'response_expectation'
                   JOIN secretary_response_expectations expectation
                     ON expectation.expectation_id = candidate.source_id
                   SET notification.delivery_status = 'suppressed',
                       notification.lease_token = NULL,
                       notification.lease_expires_at = NULL,
                       notification.last_error_code = 'response_already_resolved'
                   WHERE notification.delivery_status IN ('pending', 'failed')
                     AND expectation.expectation_status <> 'active'"#,
            ))
            .await
            .map_err(store_error)?;

        let materialized = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_response_expectations
                       (expectation_id, account_id, source_question_id, thread_id,
                        source_version, due_at_unix_secs, expectation_status)
                   SELECT UUID(), thread.account_id, question.question_id, question.thread_id,
                          1, MIN(source.occurred_at_unix_secs) + ?, 'active'
                   FROM secretary_thread_open_questions question
                   JOIN secretary_thread_question_sources question_source
                     ON question_source.question_id = question.question_id
                   JOIN secretary_source_events source
                     ON source.source_event_id = question_source.source_event_id
                   JOIN secretary_event_threads thread ON thread.thread_id = question.thread_id
                   WHERE question.status = 'open'
                     AND thread.status NOT IN ('resolved', 'closed')
                     AND source.actor_kind = 'external'
                     AND source.message_role = 'external_observation'
                     AND source.occurred_at_unix_secs + ? <= ?
                     AND NOT EXISTS (
                         SELECT 1
                         FROM secretary_source_events reply
                         JOIN secretary_conversations conversation
                           ON conversation.id = source.conversation_id
                         LEFT JOIN secretary_thread_events reply_member
                           ON reply_member.source_event_id = reply.source_event_id
                         WHERE reply.account_id = source.account_id
                           AND reply.conversation_id = source.conversation_id
                           AND (reply.occurred_at_unix_secs > source.occurred_at_unix_secs
                                OR (reply.occurred_at_unix_secs = source.occurred_at_unix_secs
                                    AND reply.source_event_id > source.source_event_id))
                           AND (
                               (reply.message_role = 'assistant_output'
                                AND reply_member.thread_id = question.thread_id)
                               OR
                               (reply.message_role = 'owner_observation'
                                AND (
                                    conversation.conversation_kind = 'private'
                                    OR reply_member.thread_id = question.thread_id
                                    OR reply.reply_to_event_id = source.source_event_id
                                ))
                           )
                     )
                   GROUP BY thread.account_id, question.question_id, question.thread_id
                   ORDER BY MIN(source.occurred_at_unix_secs), question.question_id
                   LIMIT ?"#,
                [
                    response_timeout_secs.into(),
                    response_timeout_secs.into(),
                    horizon.into(),
                    limit.into(),
                ],
            ))
            .await
            .map_err(store_error)?
            .rows_affected();

        let due = DueResponseExpectationRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT expectation.expectation_id, expectation.account_id,
                      expectation.source_version, account.source_channel,
                      account.platform_account_id, question.question_id, question.thread_id,
                      question.raised_by_actor_id,
                      conversation.conversation_kind,
                      conversation.platform_conversation_id
               FROM secretary_response_expectations expectation
               JOIN secretary_accounts account ON account.id = expectation.account_id
               JOIN secretary_thread_open_questions question
                 ON question.question_id = expectation.source_question_id
               JOIN secretary_thread_question_sources question_source
                 ON question_source.source_event_id = (
                    SELECT source_pick.source_event_id
                    FROM secretary_thread_question_sources source_pick
                    JOIN secretary_source_events event_pick
                      ON event_pick.source_event_id = source_pick.source_event_id
                    WHERE source_pick.question_id = expectation.source_question_id
                    ORDER BY event_pick.occurred_at_unix_secs, source_pick.source_event_id
                    LIMIT 1
                 )
                AND question_source.question_id = expectation.source_question_id
               JOIN secretary_source_events source
                 ON source.source_event_id = question_source.source_event_id
               JOIN secretary_conversations conversation ON conversation.id = source.conversation_id
               WHERE expectation.expectation_status = 'active'
                 AND expectation.due_at_unix_secs <= ?
               ORDER BY expectation.due_at_unix_secs, expectation.expectation_id
               LIMIT ? FOR UPDATE SKIP LOCKED"#,
            [now_unix_secs.into(), limit.into()],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?;
        let mut candidates_created = 0_u64;
        let mut requests_created = 0_u64;
        for expectation in due {
            let conversation = crate::ConversationRef::new(
                parse_conversation_kind(&expectation.conversation_kind)?,
                expectation.platform_conversation_id,
            )
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
            let production = produce_from_locked_source(
                &transaction,
                &LockedNotificationSource::ResponseExpectation {
                    account_id: expectation.account_id,
                    expectation_id: expectation.expectation_id,
                    source_version: expectation.source_version,
                    source_channel: expectation.source_channel,
                    platform_account_id: expectation.platform_account_id,
                    conversation,
                    actor_id: expectation.raised_by_actor_id,
                },
            )
            .await?;
            candidates_created += u64::from(production.candidate_created);
            requests_created += u64::from(production.request_created);
        }
        transaction.commit().await.map_err(store_error)?;
        Ok(FollowUpScanReport {
            response_expectations_materialized: materialized,
            response_expectations_resolved: resolved,
            notification_candidates_created: candidates_created,
            notification_evaluation_requests_created: requests_created,
            ..FollowUpScanReport::default()
        })
    }

    async fn scan_project_blockers(
        &self,
        now_unix_secs: i64,
        horizon_secs: i64,
        blocker_escalation_secs: i64,
        limit: u32,
    ) -> Result<FollowUpScanReport, InboundEventStoreError> {
        if !(60..=31_536_000).contains(&horizon_secs)
            || !(3_600..=31_536_000).contains(&blocker_escalation_secs)
            || !(1..=1000).contains(&limit)
        {
            return Err(InboundEventStoreError::InvalidData(
                "project blocker scan bounds are invalid".into(),
            ));
        }
        let horizon = now_unix_secs.saturating_add(horizon_secs);
        let transaction = self.db.begin().await.map_err(store_error)?;
        let materialized = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_follow_up_items
                       (follow_up_id, account_id, source_memory_fact_id, source_version,
                        reason_code, due_at_unix_secs, status)
                   SELECT UUID(), fact.account_id, fact.fact_id, 1, 'project_blocked',
                          UNIX_TIMESTAMP(fact.updated_at) + ?, 'scheduled'
                   FROM secretary_memory_facts fact
                   WHERE fact.fact_kind = 'project'
                     AND fact.fact_status = 'confirmed'
                     AND JSON_LENGTH(JSON_EXTRACT(fact.fact_json, '$.payload.data.blockers')) > 0
                     AND UNIX_TIMESTAMP(fact.updated_at) + ? <= ?
                     AND NOT EXISTS (
                         SELECT 1 FROM secretary_memory_facts successor
                         WHERE successor.supersedes_fact_id = fact.fact_id
                     )
                   ORDER BY fact.updated_at, fact.fact_id
                   LIMIT ?"#,
                [
                    blocker_escalation_secs.into(),
                    blocker_escalation_secs.into(),
                    horizon.into(),
                    limit.into(),
                ],
            ))
            .await
            .map_err(store_error)?
            .rows_affected();
        let rows = DueProjectBlockerRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT item.follow_up_id, item.account_id, item.source_version,
                      account.source_channel, account.platform_account_id
               FROM secretary_follow_up_items item
               JOIN secretary_memory_facts fact ON fact.fact_id = item.source_memory_fact_id
               JOIN secretary_accounts account ON account.id = item.account_id
               WHERE item.status = 'scheduled'
                 AND item.reason_code = 'project_blocked'
                 AND item.due_at_unix_secs <= ?
                 AND fact.fact_kind = 'project'
                 AND fact.fact_status = 'confirmed'
                 AND JSON_LENGTH(JSON_EXTRACT(fact.fact_json, '$.payload.data.blockers')) > 0
                 AND NOT EXISTS (
                     SELECT 1 FROM secretary_memory_facts successor
                     WHERE successor.supersedes_fact_id = fact.fact_id
                 )
               ORDER BY item.due_at_unix_secs, item.follow_up_id
               LIMIT ? FOR UPDATE SKIP LOCKED"#,
            [now_unix_secs.into(), limit.into()],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?;
        let mut candidates_created = 0_u64;
        let mut requests_created = 0_u64;
        for item in rows {
            let production = produce_from_locked_source(
                &transaction,
                &LockedNotificationSource::ProjectBlocker {
                    account_id: item.account_id,
                    follow_up_id: item.follow_up_id,
                    source_version: item.source_version,
                    source_channel: item.source_channel,
                    platform_account_id: item.platform_account_id,
                },
            )
            .await?;
            candidates_created += u64::from(production.candidate_created);
            requests_created += u64::from(production.request_created);
        }
        transaction.commit().await.map_err(store_error)?;
        Ok(FollowUpScanReport {
            project_blockers_materialized: materialized,
            notification_candidates_created: candidates_created,
            notification_evaluation_requests_created: requests_created,
            ..FollowUpScanReport::default()
        })
    }

    async fn reconcile_legacy_notifications(
        &self,
        config: &LegacyNotificationReconciliationConfig,
    ) -> Result<crate::LegacyNotificationReconciliationReport, InboundEventStoreError> {
        config.validate()?;
        let lease_token = uuid::Uuid::new_v4().to_string();
        let acquired = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_notification_reconciliation_leases \
                 SET lease_token = ?, lease_expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND) \
                 WHERE lease_name = 'legacy_owner_outbox_v1' \
                   AND (lease_token IS NULL OR lease_expires_at < UTC_TIMESTAMP(6))",
                [lease_token.clone().into(), config.lease_secs.into()],
            ))
            .await
            .map_err(store_error)?;
        if acquired.rows_affected() != 1 {
            return Err(InboundEventStoreError::InvalidData(
                "legacy owner notification reconciliation lease is active".into(),
            ));
        }

        let started = std::time::Instant::now();
        let deadline = std::time::Duration::from_secs(config.deadline_secs);
        let result = async {
            let mut report = crate::LegacyNotificationReconciliationReport::default();
            let mut rebuilt_sources = HashSet::new();
            loop {
                if started.elapsed() >= deadline {
                    break;
                }
                if report.rows_scanned >= u64::from(config.max_rows) {
                    let continuation = self
                        .db
                        .query_one_raw(Statement::from_string(
                            DatabaseBackend::MySql,
                            "SELECT notification_id FROM secretary_notification_outbox \
                             WHERE notification_candidate_id IS NULL \
                               AND delivery_status IN ('pending', 'failed') LIMIT 1",
                        ))
                        .await
                        .map_err(store_error)?;
                    if continuation.is_some() {
                        return Err(InboundEventStoreError::InvalidData(
                            "legacy owner notification reconciliation maximum rows exceeded".into(),
                        ));
                    }
                    break;
                }
                let transaction = self.db.begin().await.map_err(store_error)?;
                let renewed = transaction
                    .execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::MySql,
                        "UPDATE secretary_notification_reconciliation_leases \
                         SET lease_expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND) \
                         WHERE lease_name = 'legacy_owner_outbox_v1' AND lease_token = ?",
                        [config.lease_secs.into(), lease_token.clone().into()],
                    ))
                    .await
                    .map_err(store_error)?;
                if renewed.rows_affected() != 1 {
                    return Err(InboundEventStoreError::InvalidData(
                        "legacy owner notification reconciliation lease was lost".into(),
                    ));
                }
                let expired = transaction
                    .execute_raw(Statement::from_string(
                        DatabaseBackend::MySql,
                        "UPDATE secretary_notification_outbox SET delivery_status = 'unknown_commit', \
                         last_error_code = 'lease_expired_in_flight', lease_token = NULL, lease_expires_at = NULL \
                         WHERE notification_candidate_id IS NULL AND delivery_status = 'claimed' \
                           AND lease_expires_at < UTC_TIMESTAMP(6)",
                    ))
                    .await
                    .map_err(store_error)?;
                report.expired_claims_marked_unknown_commit += expired.rows_affected();

                let active = transaction
                    .query_one_raw(Statement::from_string(
                        DatabaseBackend::MySql,
                        "SELECT notification_id FROM secretary_notification_outbox \
                         WHERE notification_candidate_id IS NULL AND delivery_status = 'claimed' \
                           AND (lease_expires_at IS NULL OR lease_expires_at >= UTC_TIMESTAMP(6)) \
                         LIMIT 1 FOR UPDATE",
                    ))
                    .await
                    .map_err(store_error)?;
                if active.is_some() {
                    report.active_claimed = 1;
                    report.blocked = true;
                    transaction.commit().await.map_err(store_error)?;
                    return Ok(report);
                }

                let remaining = config
                    .max_rows
                    .saturating_sub(report.rows_scanned.min(u64::from(config.max_rows)) as u32);
                let page_size = config.page_size.min(remaining);
                let rows = LegacyOutboxRow::find_by_statement(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "SELECT notification_id, account_id, follow_up_id, agenda_item_id, agenda_version \
                     FROM secretary_notification_outbox \
                     WHERE notification_candidate_id IS NULL AND delivery_status IN ('pending', 'failed') \
                     ORDER BY created_at, notification_id LIMIT ? FOR UPDATE SKIP LOCKED",
                    [page_size.into()],
                ))
                .all(&transaction)
                .await
                .map_err(store_error)?;
                if rows.is_empty() {
                    transaction.commit().await.map_err(store_error)?;
                    break;
                }
                for row in rows {
                    let classification = classify_legacy_source(&transaction, &row).await?;
                    let error_code = match &classification {
                        LegacySourceClassification::Current(_) => "legacy_source_rebuild_pending",
                        LegacySourceClassification::Unverifiable => "legacy_source_unverifiable",
                        LegacySourceClassification::Stale => "legacy_source_stale",
                    };
                    let updated = transaction
                        .execute_raw(Statement::from_sql_and_values(
                            DatabaseBackend::MySql,
                            "UPDATE secretary_notification_outbox SET delivery_status = 'suppressed', \
                             last_error_code = ?, lease_token = NULL, lease_expires_at = NULL \
                             WHERE notification_id = ? AND notification_candidate_id IS NULL \
                               AND delivery_status IN ('pending', 'failed')",
                            [error_code.into(), row.notification_id.into()],
                        ))
                        .await
                        .map_err(store_error)?;
                    if updated.rows_affected() != 1 {
                        return Err(InboundEventStoreError::LeaseLost);
                    }
                    report.rows_scanned += 1;
                    report.legacy_outbox_suppressed += 1;
                    match classification {
                        LegacySourceClassification::Current(source) => {
                            let source_key = locked_source_key(&source);
                            let production = produce_from_locked_source(&transaction, &source).await?;
                            if rebuilt_sources.insert(source_key) {
                                report.legacy_sources_rebuilt += 1;
                            }
                            report.candidates_created += u64::from(production.candidate_created);
                            report.requests_created += u64::from(production.request_created);
                        }
                        LegacySourceClassification::Unverifiable => {
                            report.legacy_sources_unverifiable += 1;
                        }
                        LegacySourceClassification::Stale => report.sources_skipped_stale += 1,
                    }
                }
                transaction.commit().await.map_err(store_error)?;
            }
            let barrier = self
                .db
                .query_one_raw(Statement::from_string(
                    DatabaseBackend::MySql,
                    "SELECT notification_id FROM secretary_notification_outbox \
                     WHERE notification_candidate_id IS NULL AND (delivery_status IN ('pending', 'failed') \
                       OR (delivery_status = 'claimed' AND (lease_expires_at IS NULL OR lease_expires_at >= UTC_TIMESTAMP(6)))) \
                     LIMIT 1",
                ))
                .await
                .map_err(store_error)?;
            if barrier.is_some() {
                return Err(InboundEventStoreError::InvalidData(
                    "legacy owner notification reconciliation did not clear startup barrier".into(),
                ));
            }
            if started.elapsed() >= deadline {
                return Err(InboundEventStoreError::InvalidData(
                    "legacy owner notification reconciliation deadline exceeded".into(),
                ));
            }
            report.completed = true;
            Ok(report)
        }
        .await;

        let released = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_notification_reconciliation_leases \
                 SET lease_token = NULL, lease_expires_at = NULL \
                 WHERE lease_name = 'legacy_owner_outbox_v1' AND lease_token = ?",
                [lease_token.into()],
            ))
            .await
            .map_err(store_error)?;
        if released.rows_affected() != 1 {
            return Err(InboundEventStoreError::InvalidData(
                "legacy owner notification reconciliation lease release failed".into(),
            ));
        }
        result
    }

    async fn claim_due_notification(
        &self,
        account: &SourceAccountRef,
        now_unix_secs: i64,
        lease_secs: u64,
    ) -> Result<Option<ClaimedOwnerNotification>, InboundEventStoreError> {
        if !(1..=3600).contains(&lease_secs) {
            return Err(InboundEventStoreError::InvalidData(
                "notification lease_secs must be in 1..=3600".into(),
            ));
        }
        let transaction = self.db.begin().await.map_err(store_error)?;
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_notification_outbox notification
                   JOIN secretary_accounts account ON account.id = notification.account_id
                   SET notification.delivery_status = 'unknown_commit',
                       notification.last_error_code = 'lease_expired_in_flight',
                       notification.lease_token = NULL, notification.lease_expires_at = NULL
                   WHERE notification.delivery_status = 'claimed'
                     AND notification.lease_expires_at < UTC_TIMESTAMP(6)
                     AND account.source_channel = ? AND account.platform_account_id = ?"#,
                [
                    account.channel.as_str().into(),
                    account.account_id.clone().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        let Some(row) = NotificationClaimRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT notification.notification_id, notification.attempts,
                      account.source_channel, account.platform_account_id,
                      notification.scheduled_at_unix_secs,
                      notification.follow_up_id, notification.agenda_item_id,
                      candidate.source_kind AS policy_source_kind,
                      COALESCE(CAST(fact.fact_json AS CHAR), CAST(policy_fact.fact_json AS CHAR)) AS fact_json,
                      COALESCE(agenda.item_kind, policy_agenda.item_kind) AS agenda_kind,
                      COALESCE(agenda.title, policy_agenda.title) AS agenda_title,
                      policy_question.question_id AS response_question_id,
                      policy_question.thread_id AS response_thread_id,
                      policy_question.raised_by_actor_id AS response_actor_id,
                      policy_question.question AS response_question
               FROM secretary_notification_outbox notification
               JOIN secretary_accounts account ON account.id = notification.account_id
               LEFT JOIN secretary_follow_up_items item
                 ON item.follow_up_id = notification.follow_up_id
               LEFT JOIN secretary_memory_facts fact
                 ON fact.fact_id = item.source_memory_fact_id
               LEFT JOIN secretary_agenda_items agenda
                 ON agenda.item_id = notification.agenda_item_id
                AND agenda.version = notification.agenda_version
               LEFT JOIN secretary_notification_candidates candidate
                 ON candidate.notification_candidate_id = notification.notification_candidate_id
               LEFT JOIN secretary_notification_decisions policy_decision
                 ON policy_decision.notification_decision_id = notification.notification_decision_id
               LEFT JOIN secretary_notification_evaluation_requests policy_request
                 ON policy_request.evaluation_request_id = policy_decision.evaluation_request_id
               LEFT JOIN secretary_owner_bindings policy_binding
                 ON policy_binding.managed_account_id = notification.account_id
                AND policy_binding.command_account_id = notification.command_account_id
                AND policy_binding.owner_actor_id = notification.owner_actor_id
                AND policy_binding.status = 'active'
               LEFT JOIN secretary_follow_up_items policy_item
                 ON policy_item.follow_up_id = candidate.source_id
                AND candidate.source_kind = 'follow_up'
               LEFT JOIN secretary_memory_facts policy_fact
                 ON policy_fact.fact_id = policy_item.source_memory_fact_id
               LEFT JOIN secretary_agenda_items policy_agenda
                 ON policy_agenda.item_id = candidate.source_id
                AND policy_agenda.version = candidate.source_version
                AND candidate.source_kind = 'agenda'
               LEFT JOIN secretary_response_expectations policy_response
                 ON policy_response.expectation_id = candidate.source_id
                AND candidate.source_kind = 'response_expectation'
               LEFT JOIN secretary_thread_open_questions policy_question
                 ON policy_question.question_id = policy_response.source_question_id
               WHERE notification.delivery_status = 'pending'
                 AND notification.scheduled_at_unix_secs <= ?
                 AND account.source_channel = ? AND account.platform_account_id = ?
                 AND (
                       (notification.follow_up_id IS NOT NULL
                        AND item.status = 'scheduled' AND fact.fact_status = 'confirmed')
                    OR (notification.agenda_item_id IS NOT NULL
                        AND agenda.item_status = 'scheduled')
                    OR (
                        notification.notification_candidate_id IS NOT NULL
                        AND notification.notification_decision_id IS NOT NULL
                        AND notification.command_account_id IS NOT NULL
                        AND notification.owner_actor_id IS NOT NULL
                        AND policy_binding.binding_id IS NOT NULL
                        AND candidate.account_id = notification.account_id
                        AND policy_decision.notification_candidate_id = candidate.notification_candidate_id
                        AND policy_decision.evaluation_request_id = policy_request.evaluation_request_id
                        AND policy_decision.outcome = 'remind'
                        AND policy_request.notification_candidate_id = candidate.notification_candidate_id
                        AND policy_request.evaluation_generation = 1
                        AND policy_request.request_status = 'completed'
                        AND candidate.candidate_status = 'reminded'
                        AND (
                            (candidate.source_kind = 'follow_up'
                             AND policy_item.follow_up_id = candidate.source_id
                             AND policy_item.source_version = candidate.source_version
                             AND policy_item.status = 'scheduled'
                             AND policy_fact.fact_status = 'confirmed'
                             AND NOT EXISTS (
                                 SELECT 1 FROM secretary_memory_facts AS successor
                                 WHERE successor.supersedes_fact_id = policy_fact.fact_id
                             )
                             AND (
                                (policy_item.reason_code = 'commitment_due'
                                 AND policy_fact.fact_kind = 'commitment'
                                 AND JSON_UNQUOTE(JSON_EXTRACT(policy_fact.fact_json, '$.payload.data.status'))
                                     IN ('pending', 'proposed'))
                                OR (policy_item.reason_code = 'project_blocked'
                                    AND policy_fact.fact_kind = 'project'
                                    AND JSON_LENGTH(JSON_EXTRACT(policy_fact.fact_json, '$.payload.data.blockers')) > 0)
                             ))
                         OR (candidate.source_kind = 'agenda'
                             AND policy_agenda.item_id = candidate.source_id
                             AND policy_agenda.version = candidate.source_version
                             AND policy_agenda.item_status = 'scheduled')
                         OR (candidate.source_kind = 'response_expectation'
                             AND policy_response.expectation_id = candidate.source_id
                             AND policy_response.account_id = candidate.account_id
                             AND policy_response.source_version = candidate.source_version
                             AND policy_response.expectation_status = 'active'
                             AND policy_question.status = 'open')
                        )
                    )
                 )
               ORDER BY notification.scheduled_at_unix_secs, notification.notification_id
               LIMIT 1 FOR UPDATE SKIP LOCKED"#,
            [
                now_unix_secs.into(),
                account.channel.as_str().into(),
                account.account_id.clone().into(),
            ],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?
        else {
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        };
        let lease_token = NotificationLeaseToken::generate();
        let updated = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_notification_outbox
                   SET delivery_status = 'claimed', attempts = attempts + 1,
                       lease_token = ?, lease_expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND)
                   WHERE notification_id = ? AND delivery_status = 'pending'"#,
                [
                    lease_token.as_str().into(),
                    lease_secs.into(),
                    row.notification_id.clone().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if updated.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        transaction.commit().await.map_err(store_error)?;
        let content = match (
            row.follow_up_id.as_deref(),
            row.agenda_item_id.as_deref(),
            row.policy_source_kind.as_deref(),
        ) {
            (Some(_), None, None) | (None, None, Some("follow_up")) => {
                let fact_json = row.fact_json.ok_or_else(|| {
                    InboundEventStoreError::InvalidData(
                        "notification follow-up source memory is missing".into(),
                    )
                })?;
                let fact: MemoryFact = serde_json::from_str(&fact_json).map_err(|error| {
                    InboundEventStoreError::InvalidData(format!(
                        "notification source memory is invalid: {error}"
                    ))
                })?;
                match fact.payload {
                    MemoryPayload::Commitment(commitment) => {
                        OwnerNotificationContent::FollowUp { commitment }
                    }
                    MemoryPayload::Project(project) if !project.blockers.is_empty() => {
                        OwnerNotificationContent::ProjectBlocker {
                            project_key: project.project_key,
                            blockers: project.blockers.into_iter().take(10).collect(),
                        }
                    }
                    _ => {
                        return Err(InboundEventStoreError::InvalidData(
                            "notification follow-up source type is invalid".into(),
                        ));
                    }
                }
            }
            (None, Some(_), None) | (None, None, Some("agenda")) => {
                OwnerNotificationContent::Agenda {
                    kind: parse_agenda_kind(row.agenda_kind.as_deref().ok_or_else(|| {
                        InboundEventStoreError::InvalidData(
                            "notification agenda kind is missing".into(),
                        )
                    })?)?,
                    title: row.agenda_title.ok_or_else(|| {
                        InboundEventStoreError::InvalidData(
                            "notification agenda title is missing".into(),
                        )
                    })?,
                }
            }
            (None, None, Some("response_expectation")) => {
                OwnerNotificationContent::ResponseExpectation {
                    question_id: row.response_question_id.ok_or_else(|| {
                        InboundEventStoreError::InvalidData(
                            "response expectation question id is missing".into(),
                        )
                    })?,
                    thread_id: row.response_thread_id.ok_or_else(|| {
                        InboundEventStoreError::InvalidData(
                            "response expectation thread id is missing".into(),
                        )
                    })?,
                    raised_by_actor_id: row.response_actor_id.ok_or_else(|| {
                        InboundEventStoreError::InvalidData(
                            "response expectation actor id is missing".into(),
                        )
                    })?,
                    question_excerpt: row
                        .response_question
                        .ok_or_else(|| {
                            InboundEventStoreError::InvalidData(
                                "response expectation question is missing".into(),
                            )
                        })?
                        .chars()
                        .take(300)
                        .collect(),
                }
            }
            _ => {
                return Err(InboundEventStoreError::InvalidData(
                    "notification source is invalid".into(),
                ));
            }
        };
        Ok(Some(ClaimedOwnerNotification {
            notification_id: NotificationId::new(row.notification_id)
                .map_err(InboundEventStoreError::InvalidData)?,
            lease_token,
            managed_account: SourceAccountRef::new(
                parse_source(&row.source_channel)?,
                row.platform_account_id,
            )
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
            content,
            due_at_unix_secs: row.scheduled_at_unix_secs,
            attempt: row.attempts.saturating_add(1),
        }))
    }

    async fn mark_notification_delivered(
        &self,
        notification_id: &NotificationId,
        lease_token: &NotificationLeaseToken,
        platform_message_id: &str,
    ) -> Result<(), InboundEventStoreError> {
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_notification_outbox
               SET delivery_status = 'delivered', platform_message_id = ?,
                   delivered_at = UTC_TIMESTAMP(6), lease_token = NULL, lease_expires_at = NULL
               WHERE notification_id = ? AND delivery_status = 'claimed' AND lease_token = ?
                 AND lease_expires_at > UTC_TIMESTAMP(6)"#,
                [
                    platform_message_id.into(),
                    notification_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        info!(
            notification_id = notification_id.as_str(),
            "owner notification marked delivered"
        );
        Ok(())
    }

    async fn mark_notification_failed(
        &self,
        notification_id: &NotificationId,
        lease_token: &NotificationLeaseToken,
        error_code: &str,
        kind: NotificationFailureKind,
    ) -> Result<(), InboundEventStoreError> {
        let (status, retry) = match kind {
            NotificationFailureKind::Retryable => ("pending", true),
            NotificationFailureKind::Permanent => ("failed", false),
            NotificationFailureKind::UnknownCommit => ("unknown_commit", false),
        };
        let sql = if retry {
            r#"UPDATE secretary_notification_outbox
               SET delivery_status = ?, last_error_code = ?,
                   scheduled_at_unix_secs = UNIX_TIMESTAMP(UTC_TIMESTAMP())
                     + LEAST(3600, 30 * POW(2, LEAST(attempts, 7) - 1)),
                   lease_token = NULL, lease_expires_at = NULL
               WHERE notification_id = ? AND delivery_status = 'claimed' AND lease_token = ?
                 AND lease_expires_at > UTC_TIMESTAMP(6)"#
        } else {
            r#"UPDATE secretary_notification_outbox
               SET delivery_status = ?, last_error_code = ?, lease_token = NULL, lease_expires_at = NULL
               WHERE notification_id = ? AND delivery_status = 'claimed' AND lease_token = ?"#
        };
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                sql,
                [
                    status.into(),
                    error_code.into(),
                    notification_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }
        tracing::warn!(
            notification_id = notification_id.as_str(),
            status,
            error_code,
            "owner notification delivery failed"
        );
        Ok(())
    }
}

#[derive(Debug, FromQueryResult)]
struct DueFollowUpItemRow {
    follow_up_id: String,
    account_id: u64,
    source_version: u64,
    source_channel: String,
    platform_account_id: String,
}

#[derive(Debug, FromQueryResult)]
struct DueResponseExpectationRow {
    expectation_id: String,
    account_id: u64,
    source_version: u64,
    source_channel: String,
    platform_account_id: String,
    #[allow(dead_code)]
    question_id: String,
    #[allow(dead_code)]
    thread_id: String,
    raised_by_actor_id: String,
    conversation_kind: String,
    platform_conversation_id: String,
}

#[derive(Debug, FromQueryResult)]
struct DueProjectBlockerRow {
    follow_up_id: String,
    account_id: u64,
    source_version: u64,
    source_channel: String,
    platform_account_id: String,
}

#[derive(Debug)]
enum LegacySourceClassification {
    Current(LockedNotificationSource),
    Stale,
    Unverifiable,
}

async fn classify_legacy_source<C: ConnectionTrait>(
    db: &C,
    row: &LegacyOutboxRow,
) -> Result<LegacySourceClassification, InboundEventStoreError> {
    match (&row.follow_up_id, &row.agenda_item_id, row.agenda_version) {
        (Some(follow_up_id), None, None) => {
            let source = LockedFollowUpSourceRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT item.source_version, account.source_channel, account.platform_account_id \
                 FROM secretary_follow_up_items AS item \
                 INNER JOIN secretary_memory_facts AS fact \
                   ON fact.fact_id = item.source_memory_fact_id \
                 INNER JOIN secretary_accounts AS account ON account.id = item.account_id \
                 WHERE item.follow_up_id = ? AND item.account_id = ? \
                   AND item.status = 'scheduled' AND fact.fact_kind = 'commitment' \
                   AND fact.fact_status = 'confirmed' \
                   AND NOT EXISTS (SELECT 1 FROM secretary_memory_facts AS successor \
                                   WHERE successor.supersedes_fact_id = fact.fact_id) \
                   AND JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.status')) \
                       IN ('pending', 'proposed') FOR UPDATE",
                [follow_up_id.clone().into(), row.account_id.into()],
            ))
            .one(db)
            .await
            .map_err(store_error)?;
            Ok(match source {
                Some(source) => {
                    LegacySourceClassification::Current(LockedNotificationSource::FollowUp {
                        account_id: row.account_id,
                        follow_up_id: follow_up_id.clone(),
                        source_version: source.source_version,
                        source_channel: source.source_channel,
                        platform_account_id: source.platform_account_id,
                    })
                }
                None => LegacySourceClassification::Stale,
            })
        }
        (None, Some(item_id), Some(version)) => {
            let source = LockedAgendaSourceRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT account.source_channel, account.platform_account_id \
                 FROM secretary_agenda_items AS item \
                 INNER JOIN secretary_accounts AS account ON account.id = item.account_id \
                 WHERE item.item_id = ? AND item.account_id = ? AND item.version = ? \
                   AND item.item_status = 'scheduled' FOR UPDATE",
                [
                    item_id.clone().into(),
                    row.account_id.into(),
                    version.into(),
                ],
            ))
            .one(db)
            .await
            .map_err(store_error)?;
            Ok(match source {
                Some(source) => {
                    LegacySourceClassification::Current(LockedNotificationSource::Agenda {
                        account_id: row.account_id,
                        item_id: item_id.clone(),
                        version,
                        source_channel: source.source_channel,
                        platform_account_id: source.platform_account_id,
                    })
                }
                None => LegacySourceClassification::Stale,
            })
        }
        _ => Ok(LegacySourceClassification::Unverifiable),
    }
}
fn locked_source_key(source: &LockedNotificationSource) -> (u64, &'static str, String, u64) {
    match source {
        LockedNotificationSource::Agenda {
            account_id,
            item_id,
            version,
            ..
        } => (*account_id, "agenda", item_id.clone(), *version),
        LockedNotificationSource::FollowUp {
            account_id,
            follow_up_id,
            source_version,
            ..
        } => (
            *account_id,
            "follow_up",
            follow_up_id.clone(),
            *source_version,
        ),
        LockedNotificationSource::ResponseExpectation {
            account_id,
            expectation_id,
            source_version,
            ..
        } => (
            *account_id,
            "response_expectation",
            expectation_id.clone(),
            *source_version,
        ),
        LockedNotificationSource::ProjectBlocker {
            account_id,
            follow_up_id,
            source_version,
            ..
        } => (
            *account_id,
            "follow_up",
            follow_up_id.clone(),
            *source_version,
        ),
    }
}

#[derive(Debug, FromQueryResult)]
struct LockedFollowUpSourceRow {
    source_version: u64,
    source_channel: String,
    platform_account_id: String,
}

#[derive(Debug, FromQueryResult)]
struct LockedAgendaSourceRow {
    source_channel: String,
    platform_account_id: String,
}

#[derive(Debug, FromQueryResult)]
struct LegacyOutboxRow {
    notification_id: String,
    account_id: u64,
    follow_up_id: Option<String>,
    agenda_item_id: Option<String>,
    agenda_version: Option<u64>,
}

#[derive(Debug, FromQueryResult)]
struct NotificationClaimRow {
    notification_id: String,
    attempts: u32,
    source_channel: String,
    platform_account_id: String,
    scheduled_at_unix_secs: i64,
    follow_up_id: Option<String>,
    agenda_item_id: Option<String>,
    policy_source_kind: Option<String>,
    fact_json: Option<String>,
    agenda_kind: Option<String>,
    agenda_title: Option<String>,
    response_question_id: Option<String>,
    response_thread_id: Option<String>,
    response_actor_id: Option<String>,
    response_question: Option<String>,
}

fn parse_agenda_kind(value: &str) -> Result<AgendaItemKind, InboundEventStoreError> {
    match value {
        "schedule" => Ok(AgendaItemKind::Schedule),
        "task" => Ok(AgendaItemKind::Task),
        "reminder" => Ok(AgendaItemKind::Reminder),
        _ => Err(InboundEventStoreError::InvalidData(
            "notification agenda kind is invalid".into(),
        )),
    }
}

fn parse_source(value: &str) -> Result<MessageSource, InboundEventStoreError> {
    match value {
        "napcat" => Ok(MessageSource::NapCat),
        "qq_open_platform" => Ok(MessageSource::QqOpenPlatform),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown message source: {value}"
        ))),
    }
}

fn parse_conversation_kind(value: &str) -> Result<crate::ConversationKind, InboundEventStoreError> {
    match value {
        "private" => Ok(crate::ConversationKind::Private),
        "group" => Ok(crate::ConversationKind::Group),
        "owner_control" => Ok(crate::ConversationKind::OwnerControl),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown conversation kind: {value}"
        ))),
    }
}
