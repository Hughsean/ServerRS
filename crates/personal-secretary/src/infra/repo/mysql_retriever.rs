//! MySQL Retriever 仓储。实现 [`RetrieverStoreT`]。
//!
//! 查询 `secretary_source_events` + `secretary_message_contents` + `secretary_conversations`。
//! 正文摘录按内容策略返回有界长度（约束 7）。跨账号查询在 SQL 层被 `account_id` 强制过滤。

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};
use tracing::debug;

use super::mysql_inbound::store_error;
use crate::{
    ConversationKind, ConversationRef, EventQuery, EventSearchResult, InboundEventStoreError,
    MessageRole, ReferenceCandidate, ReferenceContext, RetrieverStoreT, SourceAccountRef,
    SourceEventDetail, SourceEventId, ThreadSearchResult, UpcomingItem, VerifiedActor,
    VerifiedActorKind,
};

/// 正文摘录最大字符数（约束 7）。
const EXCERPT_MAX_CHARS: u32 = 500;

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
               WHERE e.account_id = ?"#,
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
               WHERE e.source_event_id = ? AND e.account_id = ?"#,
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
            r#"SELECT e.source_event_id, e.actor_platform_id, te.thread_id,
                      SUBSTRING(m.normalized_text, 1, ?) AS excerpt
               FROM secretary_source_events e
               LEFT JOIN secretary_message_contents m ON e.source_event_id = m.source_event_id
               LEFT JOIN secretary_thread_events te ON te.source_event_id = e.source_event_id
               WHERE e.account_id = ?
                 AND (e.actor_platform_id LIKE ? OR m.normalized_text LIKE ?)
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
        Ok(rows
            .into_iter()
            .map(|r| {
                let source_event_ids = r
                    .source_event_id
                    .split(',')
                    .filter_map(|id| SourceEventId::new(id.trim()).ok())
                    .collect();
                ReferenceCandidate {
                    actor_id: r.actor_platform_id,
                    participant: None,
                    thread_id: r
                        .thread_id
                        .as_deref()
                        .and_then(|id| crate::EventThreadId::new(id).ok()),
                    source_event_ids,
                    evidence: format!("匹配表达式: {expression}"),
                }
            })
            .collect())
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
}

/// 通过 SourceAccountRef 解析 secretary_accounts.id。
async fn resolve_account_id(
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
        participant: None,
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
        participant: None,
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

/// 把领域身份错误映射为存储错误。
fn domain_err<E: std::fmt::Display>(error: E) -> InboundEventStoreError {
    InboundEventStoreError::InvalidData(error.to_string())
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
