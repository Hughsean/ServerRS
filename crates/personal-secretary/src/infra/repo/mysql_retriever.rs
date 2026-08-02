//! MySQL Retriever 仓储。实现 [`RetrieverStoreT`]。
//!
//! 查询 `secretary_source_events` + `secretary_message_contents` + `secretary_conversations`。
//! 正文摘录按内容策略返回有界长度（约束 7）。跨账号查询在 SQL 层被 `account_id` 强制过滤。

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};
use tracing::debug;

use super::mysql_inbound::store_error;
use crate::planner::AgentEventView;
use crate::{
    ContentTrustLevel, ConversationKind, ConversationRef, EventQuery, EventSearchResult,
    EventThreadId, IdentityTrust, InboundEventStoreError, MessageRole, ParticipantIdentity,
    PendingOwnerWorkItem, PlatformIdentityKind, ReferenceCandidate, ReferenceContext,
    RetrieverStoreT, SecretaryStatusView, SourceAccountRef, SourceEventDetail, SourceEventId,
    ThreadActorRef, ThreadActorSummary, ThreadClaimSummary, ThreadContextView,
    ThreadDecisionSummary, ThreadQuestionSummary, ThreadSearchResult, UpcomingItem, VerifiedActor,
    VerifiedActorKind,
};

/// 正文摘录最大字符数（约束 7）。
const EXCERPT_MAX_CHARS: u32 = 500;
/// AgentEventView 正文摘录最大字符数（Planner LLM 上下文用，比检索结果更宽）。
const EVENT_VIEW_EXCERPT_MAX_CHARS: u32 = 1_000;

pub(crate) struct MySqlRetrieverStore {
    db: DatabaseConnection,
}

impl MySqlRetrieverStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RetrieverStoreT for MySqlRetrieverStore {
    async fn search_events(
        &self,
        query: &EventQuery,
    ) -> Result<Vec<EventSearchResult>, InboundEventStoreError> {
        crate::validate_event_query(query)
            .map_err(|e| InboundEventStoreError::InvalidData(e.to_string()))?;
        // 查找 account_id
        let account_id = resolve_account_id(&self.db, &query.account).await?;
        let mut sql = String::from(
            r#"SELECT e.source_event_id, e.actor_platform_id, e.actor_kind,
                      e.message_role, e.occurred_at_unix_secs, te.thread_id,
                      e.reply_to_event_id,
                      c.platform_conversation_id, c.conversation_kind,
                      CASE
                        WHEN c.memory_mode = 'never_long_term' OR m.content_mode = 'never_long_term'
                          THEN 'never_long_term'
                        WHEN c.memory_mode = 'envelope_only' OR m.content_mode = 'envelope_only'
                          THEN 'envelope_only'
                        WHEN c.memory_mode = 'local_only' OR m.content_mode = 'local_only'
                          THEN 'local_only'
                        ELSE COALESCE(c.memory_mode, 'normal')
                      END AS memory_mode,
                      SUBSTRING(m.normalized_text, 1, ?) AS excerpt
               FROM secretary_source_events e
               INNER JOIN secretary_conversations c ON e.conversation_id = c.id
               LEFT JOIN secretary_message_contents m ON e.source_event_id = m.source_event_id
               LEFT JOIN secretary_thread_events te ON te.source_event_id = e.source_event_id
               WHERE e.account_id = ?
               AND NOT EXISTS (
                   SELECT 1 FROM secretary_message_tombstones t
                   WHERE t.source_event_id = e.source_event_id
                     AND t.account_id = e.account_id
                     AND t.status = 'applied'
               )"#,
        );
        let mut params: Vec<sea_orm::Value> = vec![EXCERPT_MAX_CHARS.into(), account_id.into()];

        if let Some(conv) = &query.conversation {
            sql.push_str(" AND c.platform_conversation_id = ? AND c.conversation_kind = ?");
            params.push(conv.id.clone().into());
            params.push(conv.kind.as_str().into());
        }
        if let Some(actor_id) = &query.actor_id {
            sql.push_str(" AND e.actor_platform_id = ?");
            params.push(actor_id.clone().into());
        }
        if let Some(thread_id) = &query.thread_id {
            sql.push_str(" AND te.thread_id = ?");
            params.push(thread_id.as_str().into());
        }
        if let Some(since) = query.since_unix_secs {
            sql.push_str(" AND e.occurred_at_unix_secs >= ?");
            params.push(since.into());
        }
        if let Some(until) = query.until_unix_secs {
            sql.push_str(" AND e.occurred_at_unix_secs <= ?");
            params.push(until.into());
        }
        if let Some(text) = &query.query_text {
            sql.push_str(" AND m.normalized_text LIKE ?");
            params.push(format!("%{text}%").into());
        }
        sql.push_str(" ORDER BY e.occurred_at_unix_secs DESC, e.source_event_id DESC LIMIT ?");
        params.push(query.limit.into());

        let rows = EventSearchRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            &sql,
            params,
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        debug!(count = rows.len(), "retriever search_events completed");
        rows.into_iter().map(map_search_row).collect()
    }

    async fn read_source_event(
        &self,
        event_id: &SourceEventId,
        account: &SourceAccountRef,
    ) -> Result<Option<SourceEventDetail>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        let row = EventDetailRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT e.source_event_id, e.actor_platform_id, e.actor_kind,
                      e.message_role, e.occurred_at_unix_secs, te.thread_id,
                      e.reply_to_event_id,
                      c.platform_conversation_id, c.conversation_kind,
                      CASE
                        WHEN c.memory_mode = 'never_long_term' OR m.content_mode = 'never_long_term'
                          THEN 'never_long_term'
                        WHEN c.memory_mode = 'envelope_only' OR m.content_mode = 'envelope_only'
                          THEN 'envelope_only'
                        WHEN c.memory_mode = 'local_only' OR m.content_mode = 'local_only'
                          THEN 'local_only'
                        ELSE COALESCE(c.memory_mode, 'normal')
                      END AS memory_mode,
                      SUBSTRING(m.normalized_text, 1, ?) AS normalized_text
               FROM secretary_source_events e
               INNER JOIN secretary_conversations c ON e.conversation_id = c.id
               LEFT JOIN secretary_message_contents m ON e.source_event_id = m.source_event_id
               LEFT JOIN secretary_thread_events te ON te.source_event_id = e.source_event_id
               WHERE e.source_event_id = ? AND e.account_id = ?
               AND NOT EXISTS (
                   SELECT 1 FROM secretary_message_tombstones t
                   WHERE t.source_event_id = e.source_event_id
                     AND t.account_id = e.account_id
                     AND t.status = 'applied'
               )"#,
            [
                EXCERPT_MAX_CHARS.into(),
                event_id.as_str().into(),
                account_id.into(),
            ],
        ))
        .one(&self.db)
        .await
        .map_err(store_error)?;
        row.map(|r| map_detail_row(r, account.clone())).transpose()
    }

    async fn search_threads(
        &self,
        account: &SourceAccountRef,
        query_text: &str,
        limit: u16,
    ) -> Result<Vec<ThreadSearchResult>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        let rows = ThreadSearchRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT t.thread_id, t.status, COUNT(te.source_event_id) AS event_count,
                      MAX(e.occurred_at_unix_secs) AS latest_at,
                      SUBSTRING((SELECT m2.normalized_text
                                 FROM secretary_source_events e2
                                 LEFT JOIN secretary_thread_events te2
                                   ON te2.source_event_id = e2.source_event_id
                                 LEFT JOIN secretary_message_contents m2
                                   ON e2.source_event_id = m2.source_event_id
                                 WHERE te2.thread_id = t.thread_id
                                   AND NOT EXISTS (
                                       SELECT 1 FROM secretary_message_tombstones t2
                                       WHERE t2.source_event_id = e2.source_event_id
                                         AND t2.account_id = e2.account_id
                                         AND t2.status = 'applied'
                                   )
                                 ORDER BY e2.occurred_at_unix_secs DESC LIMIT 1), 1, ?) AS latest_excerpt
               FROM secretary_event_threads t
               LEFT JOIN secretary_thread_events te ON te.thread_id = t.thread_id
               LEFT JOIN secretary_source_events e ON e.source_event_id = te.source_event_id
                 AND e.account_id = ?
               LEFT JOIN secretary_message_contents m ON e.source_event_id = m.source_event_id
               WHERE t.account_id = ?
                 AND (m.normalized_text LIKE ? OR t.thread_id LIKE ?)
               GROUP BY t.thread_id, t.status
               ORDER BY latest_at DESC
               LIMIT ?"#,
            [
                EXCERPT_MAX_CHARS.into(),
                account_id.into(),
                account_id.into(),
                format!("%{query_text}%").into(),
                format!("%{query_text}%").into(),
                limit.into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        rows.into_iter().map(map_thread_row).collect()
    }

    async fn find_reference_candidates(
        &self,
        account: &SourceAccountRef,
        expression: &str,
        _context: &ReferenceContext,
    ) -> Result<Vec<ReferenceCandidate>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        // 简单实现：按 actor_platform_id 或 normalized_text 模糊匹配
        let rows = ReferenceCandidateRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT e.source_event_id, e.actor_platform_id, e.actor_kind, te.thread_id,
                      SUBSTRING(m.normalized_text, 1, ?) AS excerpt
               FROM secretary_source_events e
               LEFT JOIN secretary_message_contents m ON e.source_event_id = m.source_event_id
               LEFT JOIN secretary_thread_events te ON te.source_event_id = e.source_event_id
               WHERE e.account_id = ?
                 AND (e.actor_platform_id LIKE ? OR m.normalized_text LIKE ?)
                 AND NOT EXISTS (
                     SELECT 1 FROM secretary_message_tombstones t
                     WHERE t.source_event_id = e.source_event_id
                       AND t.account_id = e.account_id
                       AND t.status = 'applied'
                 )
               ORDER BY e.occurred_at_unix_secs DESC
               LIMIT 10"#,
            [
                EXCERPT_MAX_CHARS.into(),
                account_id.into(),
                format!("%{expression}%").into(),
                format!("%{expression}%").into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        rows.into_iter()
            .map(|r| {
                let source_event_ids = r
                    .source_event_id
                    .split(',')
                    .filter_map(|id| SourceEventId::new(id.trim()).ok())
                    .collect();
                let participant = r
                    .actor_platform_id
                    .as_deref()
                    .map(|id| participant_for(&r.actor_kind, id))
                    .transpose()?;
                Ok(ReferenceCandidate {
                    actor_id: r.actor_platform_id,
                    participant,
                    thread_id: r
                        .thread_id
                        .as_deref()
                        .and_then(|id| crate::EventThreadId::new(id).ok()),
                    source_event_ids,
                    evidence: format!("匹配表达式: {expression}"),
                })
            })
            .collect::<Result<Vec<_>, InboundEventStoreError>>()
    }

    async fn list_upcoming(
        &self,
        account: &SourceAccountRef,
        horizon_secs: u64,
    ) -> Result<Vec<UpcomingItem>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        let now = Utc::now().timestamp();
        let deadline = now + horizon_secs as i64;
        // 承诺存储在 secretary_memory_facts（fact_kind='commitment'），
        // due_at_unix_secs 在 fact_json 中。用 JSON_EXTRACT 提取。
        let rows = UpcomingItemRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT fact_id AS item_id, 'commitment' AS kind,
                      CAST(JSON_EXTRACT(fact_json, '$.due_at_unix_secs') AS SIGNED) AS due_at_unix_secs,
                      SUBSTRING(JSON_UNQUOTE(JSON_EXTRACT(fact_json, '$.text')), 1, ?) AS excerpt,
                      (SELECT source_event_id FROM secretary_memory_fact_sources
                       WHERE fact_id = f.fact_id LIMIT 1) AS source_event_id
               FROM secretary_memory_facts f
               WHERE f.account_id = ?
                 AND f.fact_kind = 'commitment'
                 AND f.fact_status = 'confirmed'
                 AND JSON_EXTRACT(fact_json, '$.due_at_unix_secs') IS NOT NULL
                 AND CAST(JSON_EXTRACT(fact_json, '$.due_at_unix_secs') AS SIGNED) BETWEEN ? AND ?
               ORDER BY due_at_unix_secs ASC"#,
            [
                EXCERPT_MAX_CHARS.into(),
                account_id.into(),
                now.into(),
                deadline.into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        rows.into_iter().map(map_upcoming_row).collect()
    }

    async fn secretary_status(
        &self,
        account: &SourceAccountRef,
    ) -> Result<SecretaryStatusView, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        let row = SecretaryStatusRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT
                (SELECT COUNT(*) FROM secretary_ingestion_gaps
                 WHERE account_id = ? AND status IN ('uncertain', 'backfilling', 'unrecoverable'))
                    AS unresolved_gap_count,
                (SELECT COUNT(*) FROM secretary_ingestion_gaps
                 WHERE account_id = ? AND gap_ended_at IS NULL) AS open_gap_count,
                (SELECT CAST(UNIX_TIMESTAMP(MIN(gap_started_at)) AS SIGNED)
                 FROM secretary_ingestion_gaps
                 WHERE account_id = ? AND status IN ('uncertain', 'backfilling', 'unrecoverable'))
                    AS earliest_gap_started_at_unix_secs,
                (SELECT COUNT(*) FROM secretary_event_threads
                 WHERE account_id = ? AND status IN ('open', 'reopened')) AS open_thread_count,
                (SELECT COUNT(*) FROM secretary_event_threads
                 WHERE account_id = ? AND status = 'waiting') AS waiting_thread_count,
                (SELECT COUNT(*) FROM secretary_response_expectations
                 WHERE account_id = ? AND expectation_status = 'active')
                    AS active_response_expectation_count,
                (SELECT COUNT(*) FROM secretary_follow_up_items
                 WHERE account_id = ? AND status = 'scheduled') AS scheduled_follow_up_count,
                (SELECT COUNT(*)
                 FROM secretary_notification_evaluation_requests r
                 INNER JOIN secretary_notification_candidates c
                    ON c.notification_candidate_id = r.notification_candidate_id
                 WHERE c.account_id = ? AND r.request_status IN ('pending', 'claimed'))
                    AS pending_evaluation_count,
                (SELECT COUNT(*) FROM secretary_notification_outbox
                 WHERE account_id = ? AND delivery_status IN ('pending', 'claimed'))
                    AS pending_outbox_count,
                (SELECT COUNT(*) FROM secretary_notification_outbox
                 WHERE account_id = ? AND delivery_status IN ('failed', 'unknown_commit'))
                    AS failed_outbox_count"#,
            vec![account_id.into(); 10],
        ))
        .one(&self.db)
        .await
        .map_err(store_error)?
        .ok_or_else(|| {
            InboundEventStoreError::InvalidData("status query returned no row".into())
        })?;
        Ok(SecretaryStatusView {
            unresolved_gap_count: checked_count(row.unresolved_gap_count)?,
            open_gap_count: checked_count(row.open_gap_count)?,
            earliest_gap_started_at_unix_secs: row.earliest_gap_started_at_unix_secs,
            open_thread_count: checked_count(row.open_thread_count)?,
            waiting_thread_count: checked_count(row.waiting_thread_count)?,
            active_response_expectation_count: checked_count(
                row.active_response_expectation_count,
            )?,
            scheduled_follow_up_count: checked_count(row.scheduled_follow_up_count)?,
            pending_evaluation_count: checked_count(row.pending_evaluation_count)?,
            pending_outbox_count: checked_count(row.pending_outbox_count)?,
            failed_outbox_count: checked_count(row.failed_outbox_count)?,
        })
    }

    async fn list_pending_owner_work(
        &self,
        account: &SourceAccountRef,
        limit: u16,
    ) -> Result<Vec<PendingOwnerWorkItem>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        let rows = PendingOwnerWorkRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT source_kind, source_id, due_at_unix_secs, work_status, summary, source_version
               FROM (
                    SELECT 'response_expectation' AS source_kind,
                           expectation_id AS source_id,
                           due_at_unix_secs,
                           expectation_status AS work_status,
                           '外部联系人的问题仍待本人回复' AS summary,
                           source_version
                    FROM secretary_response_expectations
                    WHERE account_id = ? AND expectation_status = 'active'
                    UNION ALL
                    SELECT 'follow_up', f.follow_up_id, f.due_at_unix_secs, f.status,
                           SUBSTRING(CONCAT(f.reason_code, ':', m.subject_key), 1, 120),
                           f.source_version
                    FROM secretary_follow_up_items f
                    INNER JOIN secretary_memory_facts m
                        ON m.fact_id = f.source_memory_fact_id AND m.account_id = f.account_id
                    WHERE f.account_id = ? AND f.status = 'scheduled'
                    UNION ALL
                    SELECT 'agenda', item_id, scheduled_at_unix_secs, item_status,
                           SUBSTRING(title, 1, 120),
                           version AS source_version
                    FROM secretary_agenda_items
                    WHERE account_id = ? AND item_status = 'scheduled'
                    UNION ALL
                    SELECT 'outbox', notification_id, scheduled_at_unix_secs, delivery_status,
                           CONCAT('Owner 通知投递状态: ', delivery_status),
                           NULL AS source_version
                    FROM secretary_notification_outbox
                    WHERE account_id = ? AND delivery_status IN ('failed', 'unknown_commit')
               ) work
               ORDER BY due_at_unix_secs IS NULL, due_at_unix_secs, source_kind, source_id
               LIMIT ?"#,
            [
                account_id.into(),
                account_id.into(),
                account_id.into(),
                account_id.into(),
                limit.into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        Ok(rows
            .into_iter()
            .map(|row| PendingOwnerWorkItem {
                source_kind: row.source_kind,
                source_id: row.source_id,
                due_at_unix_secs: row.due_at_unix_secs,
                status: row.work_status,
                summary: row.summary.chars().take(120).collect(),
                source_version: row.source_version,
            })
            .collect())
    }

    async fn thread_context(
        &self,
        account: &SourceAccountRef,
        thread_id: &crate::EventThreadId,
    ) -> Result<Option<ThreadContextView>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        let Some(overview) = ThreadOverviewRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT t.thread_id, t.status,
                          (SELECT COUNT(*) FROM secretary_thread_events te
                           WHERE te.thread_id = t.thread_id) AS event_count
                   FROM secretary_event_threads t
                   WHERE t.thread_id = ? AND t.account_id = ?"#,
            [thread_id.as_str().into(), account_id.into()],
        ))
        .one(&self.db)
        .await
        .map_err(store_error)?
        else {
            return Ok(None);
        };

        let actors = ThreadActorRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT e.actor_kind, e.actor_platform_id, COUNT(*) AS event_count
               FROM secretary_thread_events te
               INNER JOIN secretary_source_events e ON e.source_event_id = te.source_event_id
               WHERE te.thread_id = ? AND e.account_id = ?
               GROUP BY e.actor_kind, e.actor_platform_id
               ORDER BY event_count DESC, e.actor_platform_id
               LIMIT 10"#,
            [thread_id.as_str().into(), account_id.into()],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?
        .into_iter()
        .map(|row| {
            Ok(ThreadActorSummary {
                actor_kind: row.actor_kind,
                actor_id: row.actor_platform_id,
                event_count: checked_count(row.event_count)?,
            })
        })
        .collect::<Result<Vec<_>, InboundEventStoreError>>()?;

        let claims = ThreadClaimRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT c.claim_id, c.claim_kind, c.claimant_actor_id, c.status,
                      SUBSTRING(c.statement, 1, 120) AS statement,
                      (SELECT GROUP_CONCAT(s.source_event_id ORDER BY s.source_event_id SEPARATOR ',')
                       FROM secretary_thread_claim_sources s WHERE s.claim_id = c.claim_id)
                        AS source_event_ids
               FROM secretary_thread_claims c
               WHERE c.thread_id = ?
               ORDER BY c.created_at DESC, c.claim_id DESC LIMIT 5"#,
            [thread_id.as_str().into()],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?
        .into_iter()
        .map(map_thread_claim_row)
        .collect::<Result<Vec<_>, _>>()?;

        let decisions = ThreadDecisionRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT d.decision_id, d.status, SUBSTRING(d.statement, 1, 120) AS statement,
                      (SELECT GROUP_CONCAT(s.source_event_id ORDER BY s.source_event_id SEPARATOR ',')
                       FROM secretary_thread_decision_sources s WHERE s.decision_id = d.decision_id)
                        AS source_event_ids
               FROM secretary_thread_decisions d
               WHERE d.thread_id = ?
               ORDER BY d.created_at DESC, d.decision_id DESC LIMIT 5"#,
            [thread_id.as_str().into()],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?
        .into_iter()
        .map(map_thread_decision_row)
        .collect::<Result<Vec<_>, _>>()?;

        let open_questions = ThreadQuestionRow::find_by_statement(
            Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"SELECT q.question_id, q.raised_by_actor_id, q.status,
                          SUBSTRING(q.question, 1, 120) AS question,
                          (SELECT GROUP_CONCAT(s.source_event_id ORDER BY s.source_event_id SEPARATOR ',')
                           FROM secretary_thread_question_sources s WHERE s.question_id = q.question_id)
                            AS source_event_ids
                   FROM secretary_thread_open_questions q
                   WHERE q.thread_id = ? AND q.status = 'open'
                   ORDER BY q.created_at DESC, q.question_id DESC LIMIT 5"#,
                [thread_id.as_str().into()],
            ),
        )
        .all(&self.db)
        .await
        .map_err(store_error)?
        .into_iter()
        .map(map_thread_question_row)
        .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(ThreadContextView {
            thread_id: crate::EventThreadId::new(overview.thread_id).map_err(domain_err)?,
            status: parse_thread_status(&overview.status)?,
            event_count: checked_count(overview.event_count)?,
            actors,
            claims,
            decisions,
            open_questions,
        }))
    }

    /// 列出账号最近的 N 条事件证据视图，包含发送者、@、Reply、Thread 和内容策略。
    /// 数据库先按 received_at 倒序取最近 N 条，Rust 侧再反转为时间正序。
    async fn list_recent_event_views(
        &self,
        account: &SourceAccountRef,
        limit: u16,
    ) -> Result<Vec<AgentEventView>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        let sql = format!(
            r#"SELECT e.source_event_id, e.actor_platform_id, e.actor_kind,
                      e.message_role, e.occurred_at_unix_secs, e.received_at,
                      te.thread_id, e.reply_to_event_id,
                      c.platform_conversation_id, c.conversation_kind,
                      CASE
                        WHEN c.memory_mode = 'never_long_term'
                          OR m.content_mode = 'never_long_term'
                          OR m.content_mode IS NULL
                          THEN 'never_long_term'
                        WHEN c.memory_mode = 'envelope_only' OR m.content_mode = 'envelope_only'
                          THEN 'envelope_only'
                        WHEN c.memory_mode = 'local_only' OR m.content_mode = 'local_only'
                          THEN 'local_only'
                        ELSE COALESCE(c.memory_mode, 'normal')
                      END AS memory_mode,
                      SUBSTRING(m.normalized_text, 1, {excerpt_max}) AS excerpt,
                      CAST(m.mentioned_actor_ids AS CHAR) AS mentioned_actor_ids, m.mention_all
               FROM secretary_source_events e
               INNER JOIN secretary_conversations c ON e.conversation_id = c.id
               LEFT JOIN secretary_message_contents m ON e.source_event_id = m.source_event_id
               LEFT JOIN secretary_effective_thread_events te
                   ON te.source_event_id = e.source_event_id
               WHERE e.account_id = ?
               AND NOT EXISTS (
                   SELECT 1 FROM secretary_message_tombstones t
                   WHERE t.source_event_id = e.source_event_id
                     AND t.account_id = e.account_id
                     AND t.status = 'applied'
               )
               ORDER BY e.received_at DESC, e.source_event_id DESC
               LIMIT ?"#,
            excerpt_max = EVENT_VIEW_EXCERPT_MAX_CHARS,
        );
        let mut rows = RecentEventViewRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            &sql,
            vec![account_id.into(), (limit as u64).into()],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        // 倒序查询，正序返回
        rows.reverse();
        let account_owned = account.clone();
        rows.into_iter()
            .map(|row| map_recent_event_view_row(row, &account_owned))
            .collect()
    }
}

/// 通过 SourceAccountRef 解析 secretary_accounts.id。
pub(crate) async fn resolve_account_id(
    db: &DatabaseConnection,
    account: &SourceAccountRef,
) -> Result<u64, InboundEventStoreError> {
    AccountIdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ? AND status = 'active'",
        [account.channel.as_str().into(), account.account_id.clone().into()],
    ))
    .one(db)
    .await
    .map_err(store_error)?
    .map(|r| r.id)
    .ok_or_else(|| {
        InboundEventStoreError::InvalidData(format!(
            "account not found: {}/{})",
            account.channel.as_str(),
            account.account_id
        ))
    })
}

fn map_search_row(row: EventSearchRow) -> Result<EventSearchResult, InboundEventStoreError> {
    let source_event_id = SourceEventId::new(&row.source_event_id)?;
    let conversation = ConversationRef::new(
        parse_conversation_kind(&row.conversation_kind)?,
        &row.platform_conversation_id,
    )
    .map_err(domain_err)?;
    let actor = VerifiedActor::new(parse_actor_kind(&row.actor_kind)?, &row.actor_platform_id)
        .map_err(domain_err)?;
    let message_role = parse_message_role(&row.message_role)?;
    let trust = parse_memory_mode(&row.memory_mode)?;
    let excerpt = filter_excerpt_by_trust(row.excerpt.unwrap_or_default(), trust);
    Ok(EventSearchResult {
        source_event_id,
        conversation,
        actor,
        participant: Some(participant_for(&row.actor_kind, &row.actor_platform_id)?),
        message_role,
        occurred_at_unix_secs: row.occurred_at_unix_secs,
        excerpt,
        content_trust_level: trust,
        thread_id: row
            .thread_id
            .as_deref()
            .and_then(|id| crate::EventThreadId::new(id).ok()),
    })
}

fn map_detail_row(
    row: EventDetailRow,
    account: SourceAccountRef,
) -> Result<SourceEventDetail, InboundEventStoreError> {
    let source_event_id = SourceEventId::new(&row.source_event_id)?;
    let conversation = ConversationRef::new(
        parse_conversation_kind(&row.conversation_kind)?,
        &row.platform_conversation_id,
    )
    .map_err(domain_err)?;
    let actor = VerifiedActor::new(parse_actor_kind(&row.actor_kind)?, &row.actor_platform_id)
        .map_err(domain_err)?;
    let message_role = parse_message_role(&row.message_role)?;
    let trust = parse_memory_mode(&row.memory_mode)?;
    let text = filter_excerpt_by_trust(row.normalized_text.unwrap_or_default(), trust);
    Ok(SourceEventDetail {
        source_event_id,
        account,
        conversation,
        actor,
        participant: Some(participant_for(&row.actor_kind, &row.actor_platform_id)?),
        message_role,
        occurred_at_unix_secs: row.occurred_at_unix_secs,
        normalized_text: text,
        content_trust_level: trust,
        reply_to_event_id: row
            .reply_to_event_id
            .as_deref()
            .and_then(|s| SourceEventId::new(s).ok()),
        thread_id: row
            .thread_id
            .as_deref()
            .and_then(|id| crate::EventThreadId::new(id).ok()),
    })
}

fn map_thread_row(row: ThreadSearchRow) -> Result<ThreadSearchResult, InboundEventStoreError> {
    Ok(ThreadSearchResult {
        thread_id: crate::EventThreadId::new(&row.thread_id).map_err(domain_err)?,
        status: parse_thread_status(&row.status)?,
        event_count: row.event_count as u64,
        latest_event_at_unix_secs: row.latest_at.unwrap_or(0),
        latest_excerpt: row.latest_excerpt.unwrap_or_default(),
    })
}

fn map_upcoming_row(row: UpcomingItemRow) -> Result<UpcomingItem, InboundEventStoreError> {
    Ok(UpcomingItem {
        item_id: row.item_id,
        kind: row.kind,
        due_at_unix_secs: row.due_at_unix_secs,
        excerpt: row.excerpt.unwrap_or_default(),
        source_event_id: SourceEventId::new(&row.source_event_id)?,
    })
}

/// 把 RecentEventViewRow 映射为 AgentEventView。
fn map_recent_event_view_row(
    row: RecentEventViewRow,
    account: &SourceAccountRef,
) -> Result<AgentEventView, InboundEventStoreError> {
    let source_event_id = SourceEventId::new(&row.source_event_id)?;
    let trust = parse_content_trust(&row.memory_mode)?;
    // content_trust_level 为 envelope_only/never_long_term 时清空正文
    let excerpt = filter_excerpt_by_trust(row.excerpt.unwrap_or_default(), trust);
    let actor = ThreadActorRef {
        account: account.clone(),
        actor_id: row.actor_platform_id,
    };
    // 解析 mentioned_actor_ids JSON 数组
    let mentioned_actors = parse_mentioned_actor_ids(&row.mentioned_actor_ids, account)?;
    let conversation = ConversationRef {
        kind: parse_conversation_kind(&row.conversation_kind)?,
        id: row.platform_conversation_id,
    };
    let reply_to_event_id = row
        .reply_to_event_id
        .filter(|id| !id.trim().is_empty())
        .map(|id| SourceEventId::new(&id))
        .transpose()?;
    let thread_id = row
        .thread_id
        .filter(|id| !id.trim().is_empty())
        .map(EventThreadId::new)
        .transpose()
        .map_err(domain_err)?;
    let role = parse_message_role(&row.message_role)?;
    Ok(AgentEventView {
        source_event_id,
        conversation,
        actor,
        occurred_at_unix_secs: row.occurred_at_unix_secs,
        role,
        content_trust_level: trust,
        excerpt,
        mentioned_actors,
        mention_all: row.mention_all.unwrap_or(0) != 0,
        reply_to_event_id,
        thread_id,
    })
}

/// 解析 content_mode / memory_mode 字符串为 ContentTrustLevel。
fn parse_content_trust(value: &str) -> Result<ContentTrustLevel, InboundEventStoreError> {
    match value {
        "normal" => Ok(ContentTrustLevel::Normal),
        "local_only" => Ok(ContentTrustLevel::LocalOnly),
        "envelope_only" => Ok(ContentTrustLevel::EnvelopeOnly),
        "never_long_term" => Ok(ContentTrustLevel::NeverLongTerm),
        other => Err(InboundEventStoreError::InvalidData(format!(
            "unknown content_trust_level: {other}"
        ))),
    }
}

/// 解析 mentioned_actor_ids JSON 字符串为 Vec<ThreadActorRef>。
fn parse_mentioned_actor_ids(
    raw: &Option<String>,
    account: &SourceAccountRef,
) -> Result<Vec<ThreadActorRef>, InboundEventStoreError> {
    let Some(json_str) = raw else {
        return Ok(Vec::new());
    };
    if json_str.trim().is_empty() || json_str == "null" {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = serde_json::from_str(json_str).map_err(|e| {
        InboundEventStoreError::InvalidData(format!("invalid mentioned_actor_ids JSON: {e}"))
    })?;
    Ok(ids
        .into_iter()
        .map(|actor_id| ThreadActorRef {
            account: account.clone(),
            actor_id,
        })
        .collect())
}

/// 把领域身份错误映射为存储错误。
fn domain_err<E: std::fmt::Display>(error: E) -> InboundEventStoreError {
    InboundEventStoreError::InvalidData(error.to_string())
}

/// 从 SourceEvent 的稳定发送者字段构造账号作用域的 ParticipantIdentity。
/// stable_id 必须来自平台稳定 ID（actor_platform_id）；昵称、群名片和 alias
/// 只能作为显示或指代线索，绝不能成为权限身份。Owner 的分类来自可信账号绑定
/// （actor_kind 在入站时按绑定判定），因此 Owner 用 Verified；其余角色是
/// 协议字段观察，用 Observed。无法确认稳定 ID 时调用方保留 None，不制造昵称身份。
fn participant_for(
    actor_kind: &str,
    actor_platform_id: &str,
) -> Result<ParticipantIdentity, InboundEventStoreError> {
    let kind = parse_actor_kind(actor_kind)?;
    let trust = if kind == VerifiedActorKind::Owner {
        IdentityTrust::Verified
    } else {
        IdentityTrust::Observed
    };
    ParticipantIdentity::new(
        PlatformIdentityKind::from_verified_actor_kind(kind),
        actor_platform_id,
        trust,
    )
    .map_err(domain_err)
}

/// 内容策略过滤：envelope_only/never_long_term 时清空正文（约束 7）。
fn filter_excerpt_by_trust(text: String, trust: crate::ContentTrustLevel) -> String {
    match trust {
        crate::ContentTrustLevel::Normal | crate::ContentTrustLevel::LocalOnly => text,
        crate::ContentTrustLevel::EnvelopeOnly | crate::ContentTrustLevel::NeverLongTerm => {
            String::new()
        }
    }
}

fn parse_conversation_kind(value: &str) -> Result<ConversationKind, InboundEventStoreError> {
    match value {
        "private" => Ok(ConversationKind::Private),
        "group" => Ok(ConversationKind::Group),
        "owner_control" => Ok(ConversationKind::OwnerControl),
        other => Err(InboundEventStoreError::InvalidData(format!(
            "unknown conversation_kind: {other}"
        ))),
    }
}

fn parse_actor_kind(value: &str) -> Result<VerifiedActorKind, InboundEventStoreError> {
    match value {
        "owner" => Ok(VerifiedActorKind::Owner),
        "official_bot" => Ok(VerifiedActorKind::OfficialBot),
        "external" => Ok(VerifiedActorKind::External),
        other => Err(InboundEventStoreError::InvalidData(format!(
            "unknown actor_kind: {other}"
        ))),
    }
}

fn parse_message_role(value: &str) -> Result<MessageRole, InboundEventStoreError> {
    match value {
        "owner_command" => Ok(MessageRole::OwnerCommand),
        "owner_observation" => Ok(MessageRole::OwnerObservation),
        "external_observation" => Ok(MessageRole::ExternalObservation),
        "assistant_output" => Ok(MessageRole::AssistantOutput),
        other => Err(InboundEventStoreError::InvalidData(format!(
            "unknown message_role: {other}"
        ))),
    }
}

fn parse_memory_mode(value: &str) -> Result<crate::ContentTrustLevel, InboundEventStoreError> {
    match value {
        "normal" => Ok(crate::ContentTrustLevel::Normal),
        "local_only" => Ok(crate::ContentTrustLevel::LocalOnly),
        "envelope_only" => Ok(crate::ContentTrustLevel::EnvelopeOnly),
        "never_long_term" => Ok(crate::ContentTrustLevel::NeverLongTerm),
        other => Err(InboundEventStoreError::InvalidData(format!(
            "unknown memory_mode: {other}"
        ))),
    }
}

fn parse_thread_status(value: &str) -> Result<crate::ThreadStatus, InboundEventStoreError> {
    match value {
        "open" => Ok(crate::ThreadStatus::Open),
        "waiting" => Ok(crate::ThreadStatus::Waiting),
        "resolved" => Ok(crate::ThreadStatus::Resolved),
        "closed" => Ok(crate::ThreadStatus::Closed),
        "reopened" => Ok(crate::ThreadStatus::Reopened),
        other => Err(InboundEventStoreError::InvalidData(format!(
            "unknown thread_status: {other}"
        ))),
    }
}

fn parse_source_event_id_list(
    value: Option<String>,
) -> Result<Vec<SourceEventId>, InboundEventStoreError> {
    value
        .unwrap_or_default()
        .split(',')
        .filter(|value| !value.is_empty())
        .map(SourceEventId::new)
        .collect()
}

fn checked_count(value: i64) -> Result<u64, InboundEventStoreError> {
    u64::try_from(value).map_err(|_| {
        InboundEventStoreError::InvalidData("database returned a negative aggregate count".into())
    })
}

fn map_thread_claim_row(row: ThreadClaimRow) -> Result<ThreadClaimSummary, InboundEventStoreError> {
    Ok(ThreadClaimSummary {
        claim_id: row.claim_id,
        claim_kind: row.claim_kind,
        claimant_actor_id: row.claimant_actor_id,
        status: row.status,
        statement: row.statement,
        source_event_ids: parse_source_event_id_list(row.source_event_ids)?,
    })
}

fn map_thread_decision_row(
    row: ThreadDecisionRow,
) -> Result<ThreadDecisionSummary, InboundEventStoreError> {
    Ok(ThreadDecisionSummary {
        decision_id: row.decision_id,
        status: row.status,
        statement: row.statement,
        source_event_ids: parse_source_event_id_list(row.source_event_ids)?,
    })
}

fn map_thread_question_row(
    row: ThreadQuestionRow,
) -> Result<ThreadQuestionSummary, InboundEventStoreError> {
    Ok(ThreadQuestionSummary {
        question_id: row.question_id,
        raised_by_actor_id: row.raised_by_actor_id,
        status: row.status,
        question: row.question,
        source_event_ids: parse_source_event_id_list(row.source_event_ids)?,
    })
}

#[derive(Debug, FromQueryResult)]
struct SecretaryStatusRow {
    unresolved_gap_count: i64,
    open_gap_count: i64,
    earliest_gap_started_at_unix_secs: Option<i64>,
    open_thread_count: i64,
    waiting_thread_count: i64,
    active_response_expectation_count: i64,
    scheduled_follow_up_count: i64,
    pending_evaluation_count: i64,
    pending_outbox_count: i64,
    failed_outbox_count: i64,
}

/// `source_version` 必须是 `u64`：四张来源表的版本列都是 `BIGINT UNSIGNED`，
/// 用 `i64` 反序列化会因 sqlx 类型不匹配而报错。`u64` 天然非负，
/// 类型不匹配/无法解码时经 `store_error` 返回明确错误，绝不静默取 0。
#[derive(Debug, FromQueryResult)]
struct PendingOwnerWorkRow {
    source_kind: String,
    source_id: String,
    due_at_unix_secs: Option<i64>,
    work_status: String,
    summary: String,
    source_version: Option<u64>,
}

#[derive(Debug, FromQueryResult)]
struct ThreadOverviewRow {
    thread_id: String,
    status: String,
    event_count: i64,
}

#[derive(Debug, FromQueryResult)]
struct ThreadActorRow {
    actor_kind: String,
    actor_platform_id: String,
    event_count: i64,
}

#[derive(Debug, FromQueryResult)]
struct ThreadClaimRow {
    claim_id: String,
    claim_kind: String,
    claimant_actor_id: String,
    status: String,
    statement: String,
    source_event_ids: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct ThreadDecisionRow {
    decision_id: String,
    status: String,
    statement: String,
    source_event_ids: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct ThreadQuestionRow {
    question_id: String,
    raised_by_actor_id: String,
    status: String,
    question: String,
    source_event_ids: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct AccountIdRow {
    id: u64,
}

#[allow(dead_code)]
#[derive(Debug, FromQueryResult)]
struct EventSearchRow {
    source_event_id: String,
    actor_platform_id: String,
    actor_kind: String,
    message_role: String,
    occurred_at_unix_secs: i64,
    thread_id: Option<String>,
    reply_to_event_id: Option<String>,
    platform_conversation_id: String,
    conversation_kind: String,
    memory_mode: String,
    excerpt: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct EventDetailRow {
    source_event_id: String,
    actor_platform_id: String,
    actor_kind: String,
    message_role: String,
    occurred_at_unix_secs: i64,
    thread_id: Option<String>,
    reply_to_event_id: Option<String>,
    platform_conversation_id: String,
    conversation_kind: String,
    memory_mode: String,
    normalized_text: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct ThreadSearchRow {
    thread_id: String,
    status: String,
    event_count: i64,
    latest_at: Option<i64>,
    latest_excerpt: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, FromQueryResult)]
struct ReferenceCandidateRow {
    source_event_id: String,
    actor_platform_id: Option<String>,
    actor_kind: String,
    thread_id: Option<String>,
    excerpt: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct UpcomingItemRow {
    item_id: String,
    kind: String,
    due_at_unix_secs: i64,
    excerpt: Option<String>,
    source_event_id: String,
}

/// list_recent_event_views 的行模型。包含 mentioned_actor_ids JSON 和 mention_all。
/// actor_kind/received_at 由 SQL 选出但仅用于 FromQueryResult 列匹配/排序。
#[allow(dead_code)]
#[derive(Debug, FromQueryResult)]
struct RecentEventViewRow {
    source_event_id: String,
    actor_platform_id: String,
    actor_kind: String,
    message_role: String,
    occurred_at_unix_secs: i64,
    received_at: chrono::NaiveDateTime,
    thread_id: Option<String>,
    reply_to_event_id: Option<String>,
    platform_conversation_id: String,
    conversation_kind: String,
    memory_mode: String,
    excerpt: Option<String>,
    mentioned_actor_ids: Option<String>,
    mention_all: Option<i8>,
}
