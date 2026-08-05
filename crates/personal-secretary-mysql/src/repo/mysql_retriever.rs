//! MySQL Retriever 仓储。实现 [`RetrieverStoreT`]。
//!
//! 查询 `secretary_source_events` + `secretary_message_contents` + `secretary_conversations`。
//! 正文摘录按内容策略返回有界长度（约束 7）。跨账号查询在 SQL 层被 `account_id` 强制过滤。

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};
use tracing::debug;

use super::mysql_inbound::store_error;
use crate::AgentEventView;
use crate::{
    AccountScopedParticipantRef, CausalEventRef, CausalThreadRef, ContentTrustLevel,
    ConversationKind, ConversationRef, EventCausalContextView, EventParticipantSummary, EventQuery,
    EventRelation, EventRelationKind, EventSearchResult, EventThreadId, GroupRole, IdentityTrust,
    InboundEventStoreError, MAX_CAUSAL_MENTIONED, MAX_CAUSAL_PARTICIPANTS, MAX_CAUSAL_SOURCE_REFS,
    MAX_PARTICIPANT_ALIASES, MAX_PARTICIPANT_ATTRIBUTES, MAX_RELATION_SOURCES, MessageRole,
    ParticipantAttribute, ParticipantAttributeKind, ParticipantContextView, ParticipantIdentity,
    PendingOwnerWorkItem, PlatformIdentityKind, ReferenceCandidate, ReferenceContext,
    RetrievalVisibility, RetrieverStoreT, SecretaryStatusView, SourceAccountRef, SourceEventDetail,
    SourceEventId, ThreadActorRef, ThreadActorSummary, ThreadClaimSummary, ThreadContextView,
    ThreadDecisionId, ThreadDecisionRevisionCursor, ThreadDecisionRevisionPage,
    ThreadDecisionSummary, ThreadQuestionSummary, ThreadSearchResult, ThreadStatus, UpcomingItem,
    VerifiedActor, VerifiedActorKind,
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

    /// participant_context 共享主体：身份种类与档案行已确定，执行证据读取与
    /// 上下文组装（宽松按 ID 查询与按完整引用查询共用）。
    #[allow(clippy::too_many_arguments)]
    async fn participant_context_impl(
        &self,
        account: &SourceAccountRef,
        account_id: u64,
        identity_kind: &str,
        actor_id: &str,
        conversation: Option<&ConversationRef>,
        thread_id: Option<&EventThreadId>,
        profile: Option<ProfileRow>,
    ) -> Result<Option<ParticipantContextView>, InboundEventStoreError> {
        let related_event_ids = load_related_event_ids(&self.db, account_id, actor_id).await?;
        let person_memory = load_confirmed_person_memory(&self.db, account_id, actor_id).await?;

        let identity_trust = profile
            .as_ref()
            .map(|profile| parse_identity_trust(&profile.trust))
            .transpose()?
            .unwrap_or(IdentityTrust::Observed);
        let kind = parse_actor_kind(identity_kind)?;
        let view_participant = AccountScopedParticipantRef::new(
            account.clone(),
            PlatformIdentityKind::from_verified_actor_kind(kind),
            actor_id.to_owned(),
            identity_trust,
        )
        .map_err(domain_err)?;

        // ---- 会话作用域观察（P0-2）----
        // conversation 参数优先；否则由线程根事件所在会话推导；两者皆无时群属性
        // 返回未知，绝不跨会话猜测（同一 Actor 在不同群的名片/角色互不污染）。
        let observation_conversation_id = match conversation {
            Some(conversation) => {
                resolve_conversation_id(&self.db, account_id, conversation).await?
            }
            None => match thread_id {
                Some(thread_id) => {
                    resolve_thread_conversation_id(&self.db, account_id, thread_id).await?
                }
                None => None,
            },
        };
        let observation = match observation_conversation_id {
            Some(conversation_id) => {
                load_conversation_observation(
                    &self.db,
                    account_id,
                    conversation_id,
                    identity_kind,
                    actor_id,
                )
                .await?
            }
            None => None,
        };
        // 观察失效闭环：历史来源整体有效 + 当前名片/角色的建立事件独立有效。
        // 来源列表有界且淘汰最旧后可能不含建立事件，因此当前值必须单独校验
        // established_by_event_id，不能只依赖历史来源列表。
        let observation_established_valid = match &observation {
            Some(observation) => match observation.established_by_event_id.as_deref() {
                Some(id) => single_event_valid(&self.db, account_id, id).await?,
                None => false,
            },
            None => false,
        };
        let observation_valid = match &observation {
            Some(observation) => {
                source_refs_valid(&self.db, account_id, &observation.source_event_ids_json).await?
                    && observation_established_valid
            }
            None => false,
        };
        let group_card = if observation_valid {
            observation.as_ref().and_then(|o| o.group_card.clone())
        } else {
            None
        };
        let group_role = if observation_valid {
            observation
                .as_ref()
                .map(|o| GroupRole::parse_protocol(Some(&o.group_role)))
                .unwrap_or(GroupRole::Unknown)
        } else {
            GroupRole::Unknown
        };

        // ---- 档案失效闭环（P0-3）----
        // 档案行被标记失效或任一来源已撤回/投影缺失/降级为 never_long_term 时，
        // 不得把旧显示名/别名/档案属性当有效事实返回。
        let profile_valid = match &profile {
            Some(profile) if profile.invalidated == 0 => {
                profile_source_refs_valid(&self.db, account_id, &profile.source_event_ids_json)
                    .await?
            }
            _ => false,
        };
        // 当前显示名由 established_by_event_id 建立（来源列表可能已把它淘汰），
        // 该建立事件失效（撤回/删除/降级）时显示名必须独立失效。
        let established_valid = match &profile {
            Some(profile) => match profile.established_by_event_id.as_deref() {
                Some(id) => profile_event_valid(&self.db, account_id, id).await?,
                None => false,
            },
            None => false,
        };
        // display 值只在档案整体有效且建立事件有效时才返回。
        let display_name = if profile_valid && established_valid {
            profile.as_ref().and_then(|p| {
                if p.display_name.trim().is_empty() {
                    None
                } else {
                    Some(p.display_name.clone())
                }
            })
        } else {
            None
        };
        let mut aliases: Vec<String> = if profile_valid {
            profile
                .as_ref()
                .map(|p| parse_aliases(&p.aliases_json))
                .transpose()?
                .unwrap_or_default()
                .into_iter()
                .take(MAX_PARTICIPANT_ALIASES)
                .collect()
        } else {
            Vec::new()
        };
        aliases.truncate(MAX_PARTICIPANT_ALIASES);

        // 每条属性独立携带来源与失效状态；召回/失效的来源不得支撑新事实。
        // DisplayName/GroupCard 的来源精确引用建立事件，而非有界历史列表。
        let mut attributes: Vec<ParticipantAttribute> = Vec::new();
        if let (Some(profile), Some(display)) = (&profile, display_name.as_ref())
            && profile_valid
        {
            let display_sources = profile
                .established_by_event_id
                .as_deref()
                .and_then(|id| SourceEventId::new(id).ok())
                .map(|id| vec![id])
                .unwrap_or_default();
            attributes.push(ParticipantAttribute {
                kind: ParticipantAttributeKind::DisplayName,
                value: display.chars().take(200).collect(),
                trust: identity_trust,
                confirmed: profile.confirmed != 0,
                source_event_ids: display_sources,
                directory_snapshot_id: None,
                invalidated: false,
                invalidation_reason: None,
            });
        }
        if observation_valid
            && let (Some(card), Some(observation)) = (group_card.as_ref(), &observation)
        {
            let card_sources = observation
                .established_by_event_id
                .as_deref()
                .and_then(|id| SourceEventId::new(id).ok())
                .map(|id| vec![id])
                .unwrap_or_default();
            attributes.push(ParticipantAttribute {
                kind: ParticipantAttributeKind::GroupCard,
                value: card.chars().take(200).collect(),
                trust: IdentityTrust::Observed,
                confirmed: false,
                source_event_ids: card_sources,
                directory_snapshot_id: None,
                invalidated: false,
                invalidation_reason: None,
            });
        }
        // MEM-002：已确认人物记忆补充关系/职责/沟通偏好；未批准的候选绝不进入确认字段。
        for fact in person_memory {
            for attribute in fact.into_attributes() {
                if attributes.len() < MAX_PARTICIPANT_ATTRIBUTES {
                    attributes.push(attribute);
                }
            }
        }
        // 权限描述：v1 无权限记忆生产者；只有账号绑定的 Owner 由身份派生，不伪造来源。
        // 权限边界由 check_participant_permission_boundary 守卫。

        let expired_or_invalidated = profile.is_some() && !profile_valid;

        Ok(Some(ParticipantContextView {
            participant: view_participant,
            display_name,
            group_card,
            aliases,
            group_role,
            attributes,
            conversation: conversation.cloned(),
            thread_id: thread_id.cloned(),
            related_event_ids,
            unresolved_ambiguity: false, // 稳定 actor_id 显式查询，无命名歧义。
            expired_or_invalidated,
        }))
    }
}

#[async_trait]
impl RetrieverStoreT for MySqlRetrieverStore {
    async fn search_events(
        &self,
        query: &EventQuery,
        visibility: RetrievalVisibility,
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
               AND (
                   (c.memory_mode = 'normal' AND m.content_mode = 'normal')
                   OR (? AND c.memory_mode IN ('normal', 'local_only')
                           AND m.content_mode IN ('normal', 'local_only'))
               )
               AND NOT EXISTS (
                   SELECT 1 FROM secretary_message_tombstones t
                   WHERE t.source_event_id = e.source_event_id
                     AND t.account_id = e.account_id
                     AND t.status = 'applied'
               )"#,
        );
        let mut params: Vec<sea_orm::Value> = vec![
            EXCERPT_MAX_CHARS.into(),
            account_id.into(),
            visibility.includes_local_only().into(),
        ];

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
            // CMD-009 目标 B：LIKE 通配符（% / _）与转义符全部转义，Owner 文本不能
            // 改变匹配范围；排序按确定性四级：硬过滤（WHERE）→ 文本相关性等级 →
            // occurred_at DESC → source_event_id DESC。
            // 相关性：2 = normalized_text 以查询文本开头（前缀命中），1 = 包含命中。
            let escaped = escape_like_pattern(text);
            let prefix = format!("{escaped}%");
            let contains = format!("%{escaped}%");
            sql.push_str(" AND m.normalized_text LIKE ? ESCAPE '\\\\'");
            params.push(contains.clone().into());
            sql.push_str(
                " ORDER BY \
                 CASE \
                   WHEN m.normalized_text LIKE ? ESCAPE '\\\\' THEN 2 \
                   WHEN m.normalized_text LIKE ? ESCAPE '\\\\' THEN 1 \
                   ELSE 0 \
                 END DESC, \
                 e.occurred_at_unix_secs DESC, e.source_event_id DESC LIMIT ?",
            );
            params.push(prefix.into());
            params.push(contains.into());
            params.push(query.limit.into());
        } else {
            sql.push_str(" ORDER BY e.occurred_at_unix_secs DESC, e.source_event_id DESC LIMIT ?");
            params.push(query.limit.into());
        }

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
        visibility: RetrievalVisibility,
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
               AND (
                   (c.memory_mode = 'normal' AND m.content_mode = 'normal')
                   OR (? AND c.memory_mode IN ('normal', 'local_only')
                           AND m.content_mode IN ('normal', 'local_only'))
                   OR ?
               )
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
                visibility.includes_local_only().into(),
                visibility.includes_internal_metadata().into(),
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
        visibility: RetrievalVisibility,
    ) -> Result<Vec<ThreadSearchResult>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        let escaped = escape_like_pattern(query_text.trim());
        let prefix = format!("{escaped}%");
        let contains = format!("%{escaped}%");
        let rows = ThreadSearchRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"WITH base_events AS (
                   SELECT ev.thread_id, t.status, e.source_event_id,
                          e.actor_platform_id, e.actor_kind, e.occurred_at_unix_secs,
                          c.platform_conversation_id, c.conversation_kind,
                          CASE
                            WHEN c.memory_mode = 'never_long_term'
                              OR m.content_mode = 'never_long_term' THEN 'never_long_term'
                            WHEN c.memory_mode = 'envelope_only'
                              OR m.content_mode = 'envelope_only' THEN 'envelope_only'
                            WHEN c.memory_mode = 'local_only'
                              OR m.content_mode = 'local_only' THEN 'local_only'
                            ELSE 'normal'
                          END AS memory_mode,
                          m.normalized_text
                   FROM secretary_effective_thread_events ev
                   JOIN secretary_event_threads t ON t.thread_id = ev.thread_id
                   JOIN secretary_source_events e ON e.source_event_id = ev.source_event_id
                   JOIN secretary_conversations c ON c.id = e.conversation_id
                   JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
                   WHERE t.account_id = ? AND e.account_id = ?
                     AND NOT EXISTS (
                         SELECT 1 FROM secretary_message_tombstones mt
                         WHERE mt.source_event_id = e.source_event_id
                           AND mt.account_id = e.account_id
                           AND mt.status = 'applied'
                     )
               ), eligible_events AS (
                   SELECT * FROM base_events
                   WHERE memory_mode = 'normal' OR (? AND memory_mode = 'local_only')
               ), thread_totals AS (
                   SELECT thread_id, COUNT(*) AS event_count,
                          MAX(occurred_at_unix_secs) AS latest_at
                   FROM eligible_events GROUP BY thread_id
               ), ranked_matches AS (
                   SELECT eligible_events.*,
                          CASE
                            WHEN normalized_text = ? THEN 3
                            WHEN normalized_text LIKE ? ESCAPE '\\' THEN 2
                            ELSE 1
                          END AS match_rank,
                          ROW_NUMBER() OVER (
                            PARTITION BY thread_id
                            ORDER BY
                              CASE
                                WHEN normalized_text = ? THEN 3
                                WHEN normalized_text LIKE ? ESCAPE '\\' THEN 2
                                ELSE 1
                              END DESC,
                              occurred_at_unix_secs DESC,
                              source_event_id DESC
                          ) AS row_rank
                   FROM eligible_events
                   WHERE normalized_text LIKE ? ESCAPE '\\'
               )
               SELECT r.thread_id, r.status, totals.event_count, totals.latest_at,
                      r.source_event_id AS representative_source_event_id,
                      r.actor_platform_id, r.actor_kind,
                      r.occurred_at_unix_secs AS representative_occurred_at,
                      r.platform_conversation_id, r.conversation_kind,
                      r.memory_mode,
                      SUBSTRING(r.normalized_text, 1, ?) AS representative_excerpt,
                      r.match_rank
               FROM ranked_matches r
               JOIN thread_totals totals ON totals.thread_id = r.thread_id
               WHERE r.row_rank = 1
               ORDER BY r.match_rank DESC, totals.latest_at DESC, r.thread_id ASC
               LIMIT ?"#,
            [
                account_id.into(),
                account_id.into(),
                visibility.includes_local_only().into(),
                query_text.trim().into(),
                prefix.clone().into(),
                query_text.trim().into(),
                prefix.into(),
                contains.into(),
                EXCERPT_MAX_CHARS.into(),
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
        context: &ReferenceContext,
    ) -> Result<Vec<ReferenceCandidate>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        // CMD-010 防线 C：非显式引用只能在当前作用域内解析。作用域 =
        // Owner 显式提供的会话/线程（context.current_conversation /
        // current_thread_id，均由已登记引用解析而来）。两者都无时返回空候选，
        // 由用例层生成 OpenReference/澄清响应 —— 绝不按"最新一个"或
        // "全局最相似一个"在账号内跨群模糊匹配。
        let conversation_id = match &context.current_conversation {
            Some(conversation) => {
                match resolve_conversation_id(&self.db, account_id, conversation).await? {
                    Some(id) => Some(id),
                    // 显式会话已失效或不属于账号时不得静默退化为仅按 Thread 查询。
                    None => return Ok(Vec::new()),
                }
            }
            None => None,
        };
        let scoped_thread_id = context
            .current_thread_id
            .as_ref()
            .map(EventThreadId::as_str);
        if conversation_id.is_none() && scoped_thread_id.is_none() {
            return Ok(Vec::new());
        }
        let escaped = escape_like_pattern(expression.trim());
        let contains = format!("%{escaped}%");
        let rows = ReferenceCandidateRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT e.source_event_id, e.actor_platform_id, e.actor_kind, te.thread_id,
                      SUBSTRING(m.normalized_text, 1, ?) AS excerpt
               FROM secretary_source_events e
               LEFT JOIN secretary_message_contents m ON e.source_event_id = m.source_event_id
               LEFT JOIN secretary_effective_thread_events te
                 ON te.source_event_id = e.source_event_id
               INNER JOIN secretary_conversations c ON c.id = e.conversation_id
               WHERE e.account_id = ?
                  AND (e.actor_platform_id LIKE ? ESCAPE '\\'
                       OR m.normalized_text LIKE ? ESCAPE '\\')
                  AND (? IS NULL OR e.conversation_id = ?)
                  AND (? IS NULL OR te.thread_id = ?)
                  AND m.source_event_id IS NOT NULL
                  AND c.memory_mode = 'normal'
                  AND m.content_mode = 'normal'
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
                contains.clone().into(),
                contains.into(),
                // NULL 参数用 Option 内的 None（sea-query 1.x Value 无 Null 变体）。
                conversation_id
                    .map(sea_orm::Value::from)
                    .unwrap_or(sea_orm::Value::BigUnsigned(None)),
                conversation_id
                    .map(sea_orm::Value::from)
                    .unwrap_or(sea_orm::Value::BigUnsigned(None)),
                scoped_thread_id
                    .map(|tid| tid.to_owned().into())
                    .unwrap_or(sea_orm::Value::String(None)),
                scoped_thread_id
                    .map(|tid| tid.to_owned().into())
                    .unwrap_or(sea_orm::Value::String(None)),
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
                 AND EXISTS (SELECT 1 FROM secretary_memory_fact_sources fs0
                             WHERE fs0.fact_id = f.fact_id)
                 AND NOT EXISTS (
                     SELECT 1 FROM secretary_memory_fact_sources fs
                     LEFT JOIN secretary_source_events se
                       ON se.source_event_id = fs.source_event_id AND se.account_id = f.account_id
                     LEFT JOIN secretary_conversations c ON c.id = se.conversation_id
                     LEFT JOIN secretary_message_contents mc ON mc.source_event_id = se.source_event_id
                     LEFT JOIN secretary_message_tombstones mt
                       ON mt.source_event_id = se.source_event_id
                      AND mt.account_id = se.account_id AND mt.status = 'applied'
                     WHERE fs.fact_id = f.fact_id
                       AND (se.source_event_id IS NULL OR c.memory_mode <> 'normal'
                            OR mc.source_event_id IS NULL OR mc.content_mode <> 'normal'
                            OR mt.source_event_id IS NOT NULL)
                 )
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
                          (SELECT COUNT(*)
                           FROM secretary_effective_thread_events te
                           JOIN secretary_source_events e ON e.source_event_id = te.source_event_id
                           JOIN secretary_conversations c ON c.id = e.conversation_id
                           JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
                           WHERE te.thread_id = t.thread_id
                             AND c.memory_mode = 'normal' AND m.content_mode = 'normal'
                             AND NOT EXISTS (
                                 SELECT 1 FROM secretary_message_tombstones mt
                                 WHERE mt.source_event_id = e.source_event_id
                                   AND mt.account_id = e.account_id AND mt.status = 'applied'
                             )) AS event_count
                   FROM secretary_event_threads t
                   WHERE t.thread_id = ? AND t.account_id = ?
                     AND EXISTS (
                         SELECT 1
                         FROM secretary_effective_thread_events te
                         JOIN secretary_source_events e ON e.source_event_id = te.source_event_id
                         JOIN secretary_conversations c ON c.id = e.conversation_id
                         JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
                         WHERE te.thread_id = t.thread_id
                           AND c.memory_mode = 'normal' AND m.content_mode = 'normal'
                           AND NOT EXISTS (
                               SELECT 1 FROM secretary_message_tombstones mt
                               WHERE mt.source_event_id = e.source_event_id
                                 AND mt.account_id = e.account_id AND mt.status = 'applied'
                           ))"#,
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
               FROM secretary_effective_thread_events te
               INNER JOIN secretary_source_events e ON e.source_event_id = te.source_event_id
               INNER JOIN secretary_conversations c ON c.id = e.conversation_id
               INNER JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
               WHERE te.thread_id = ? AND e.account_id = ?
                 AND c.memory_mode = 'normal' AND m.content_mode = 'normal'
                 AND NOT EXISTS (
                     SELECT 1 FROM secretary_message_tombstones mt
                     WHERE mt.source_event_id = e.source_event_id
                       AND mt.account_id = e.account_id AND mt.status = 'applied'
                 )
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
                 AND EXISTS (SELECT 1 FROM secretary_thread_claim_sources s0
                             WHERE s0.claim_id = c.claim_id)
                 AND NOT EXISTS (
                     SELECT 1 FROM secretary_thread_claim_sources s
                     LEFT JOIN secretary_source_events e ON e.source_event_id = s.source_event_id
                     LEFT JOIN secretary_conversations cv ON cv.id = e.conversation_id
                     LEFT JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
                     LEFT JOIN secretary_message_tombstones mt
                       ON mt.source_event_id = e.source_event_id
                      AND mt.account_id = e.account_id AND mt.status = 'applied'
                     WHERE s.claim_id = c.claim_id
                       AND (e.source_event_id IS NULL OR cv.memory_mode <> 'normal'
                            OR m.source_event_id IS NULL OR m.content_mode <> 'normal'
                            OR mt.source_event_id IS NOT NULL)
                 )
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
                      d.confidence_bps, d.supersedes_id,
                      TIMESTAMPDIFF(MICROSECOND, '1970-01-01 00:00:00', d.created_at)
                        AS created_at_unix_micros,
                      (SELECT GROUP_CONCAT(s.source_event_id ORDER BY s.source_event_id SEPARATOR ',')
                       FROM secretary_thread_decision_sources s WHERE s.decision_id = d.decision_id)
                        AS source_event_ids
               FROM secretary_thread_decisions d
               WHERE d.thread_id = ?
                 AND EXISTS (SELECT 1 FROM secretary_thread_decision_sources s0
                             WHERE s0.decision_id = d.decision_id)
                 AND NOT EXISTS (
                     SELECT 1 FROM secretary_thread_decision_sources s
                     LEFT JOIN secretary_source_events e ON e.source_event_id = s.source_event_id
                     LEFT JOIN secretary_conversations cv ON cv.id = e.conversation_id
                     LEFT JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
                     LEFT JOIN secretary_message_tombstones mt
                       ON mt.source_event_id = e.source_event_id
                      AND mt.account_id = e.account_id AND mt.status = 'applied'
                     WHERE s.decision_id = d.decision_id
                       AND (e.source_event_id IS NULL OR cv.memory_mode <> 'normal'
                            OR m.source_event_id IS NULL OR m.content_mode <> 'normal'
                            OR mt.source_event_id IS NOT NULL)
                 )
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
                     AND EXISTS (SELECT 1 FROM secretary_thread_question_sources s0
                                 WHERE s0.question_id = q.question_id)
                     AND NOT EXISTS (
                         SELECT 1 FROM secretary_thread_question_sources s
                         LEFT JOIN secretary_source_events e ON e.source_event_id = s.source_event_id
                         LEFT JOIN secretary_conversations cv ON cv.id = e.conversation_id
                         LEFT JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
                         LEFT JOIN secretary_message_tombstones mt
                           ON mt.source_event_id = e.source_event_id
                          AND mt.account_id = e.account_id AND mt.status = 'applied'
                         WHERE s.question_id = q.question_id
                           AND (e.source_event_id IS NULL OR cv.memory_mode <> 'normal'
                                OR m.source_event_id IS NULL OR m.content_mode <> 'normal'
                                OR mt.source_event_id IS NOT NULL)
                     )
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

    async fn thread_decision_revisions(
        &self,
        account: &SourceAccountRef,
        thread_id: &EventThreadId,
        cursor: Option<&ThreadDecisionRevisionCursor>,
        limit: u16,
    ) -> Result<ThreadDecisionRevisionPage, InboundEventStoreError> {
        if !(1..=50).contains(&limit) {
            return Err(InboundEventStoreError::InvalidData(
                "decision revision page limit must be in 1..=50".into(),
            ));
        }
        if cursor.is_some_and(|cursor| cursor.thread_id() != thread_id) {
            return Err(InboundEventStoreError::InvalidData(
                "decision revision cursor belongs to another thread".into(),
            ));
        }

        let account_id = resolve_account_id(&self.db, account).await?;
        let fetch_limit = u64::from(limit) + 1;
        let (sql, values) = match cursor {
            Some(cursor) => (
                r#"SELECT d.decision_id, d.status, d.statement, d.confidence_bps,
                          d.supersedes_id,
                          TIMESTAMPDIFF(MICROSECOND, '1970-01-01 00:00:00', d.created_at)
                            AS created_at_unix_micros,
                          (SELECT GROUP_CONCAT(s.source_event_id ORDER BY s.source_event_id SEPARATOR ',')
                           FROM secretary_thread_decision_sources s
                           WHERE s.decision_id = d.decision_id) AS source_event_ids
                   FROM secretary_thread_decisions d
                   INNER JOIN secretary_event_threads t ON t.thread_id = d.thread_id
                   WHERE d.thread_id = ? AND t.account_id = ?
                     AND EXISTS (SELECT 1 FROM secretary_thread_decision_sources s0
                                 WHERE s0.decision_id = d.decision_id)
                     AND NOT EXISTS (
                         SELECT 1 FROM secretary_thread_decision_sources s
                         LEFT JOIN secretary_source_events e ON e.source_event_id = s.source_event_id
                         LEFT JOIN secretary_conversations c ON c.id = e.conversation_id
                         LEFT JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
                         LEFT JOIN secretary_message_tombstones mt
                           ON mt.source_event_id = e.source_event_id
                          AND mt.account_id = e.account_id AND mt.status = 'applied'
                         WHERE s.decision_id = d.decision_id
                           AND (e.source_event_id IS NULL OR c.memory_mode <> 'normal'
                                OR m.source_event_id IS NULL OR m.content_mode <> 'normal'
                                OR mt.source_event_id IS NOT NULL)
                     )
                     AND (d.created_at < TIMESTAMPADD(MICROSECOND, ?, '1970-01-01 00:00:00')
                          OR (d.created_at = TIMESTAMPADD(MICROSECOND, ?, '1970-01-01 00:00:00')
                              AND d.decision_id < ?))
                   ORDER BY d.created_at DESC, d.decision_id DESC
                   LIMIT ?"#,
                vec![
                    thread_id.as_str().into(),
                    account_id.into(),
                    cursor.created_at_unix_micros().into(),
                    cursor.created_at_unix_micros().into(),
                    cursor.decision_id().as_str().into(),
                    fetch_limit.into(),
                ],
            ),
            None => (
                r#"SELECT d.decision_id, d.status, d.statement, d.confidence_bps,
                          d.supersedes_id,
                          TIMESTAMPDIFF(MICROSECOND, '1970-01-01 00:00:00', d.created_at)
                            AS created_at_unix_micros,
                          (SELECT GROUP_CONCAT(s.source_event_id ORDER BY s.source_event_id SEPARATOR ',')
                           FROM secretary_thread_decision_sources s
                           WHERE s.decision_id = d.decision_id) AS source_event_ids
                   FROM secretary_thread_decisions d
                   INNER JOIN secretary_event_threads t ON t.thread_id = d.thread_id
                   WHERE d.thread_id = ? AND t.account_id = ?
                     AND EXISTS (SELECT 1 FROM secretary_thread_decision_sources s0
                                 WHERE s0.decision_id = d.decision_id)
                     AND NOT EXISTS (
                         SELECT 1 FROM secretary_thread_decision_sources s
                         LEFT JOIN secretary_source_events e ON e.source_event_id = s.source_event_id
                         LEFT JOIN secretary_conversations c ON c.id = e.conversation_id
                         LEFT JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
                         LEFT JOIN secretary_message_tombstones mt
                           ON mt.source_event_id = e.source_event_id
                          AND mt.account_id = e.account_id AND mt.status = 'applied'
                         WHERE s.decision_id = d.decision_id
                           AND (e.source_event_id IS NULL OR c.memory_mode <> 'normal'
                                OR m.source_event_id IS NULL OR m.content_mode <> 'normal'
                                OR mt.source_event_id IS NOT NULL)
                     )
                   ORDER BY d.created_at DESC, d.decision_id DESC
                   LIMIT ?"#,
                vec![
                    thread_id.as_str().into(),
                    account_id.into(),
                    fetch_limit.into(),
                ],
            ),
        };
        let mut rows = ThreadDecisionRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        let has_more = rows.len() > usize::from(limit);
        if has_more {
            rows.pop();
        }
        let next_cursor = if has_more {
            rows.last()
                .map(|row| {
                    ThreadDecisionRevisionCursor::new(
                        thread_id.clone(),
                        row.created_at_unix_micros,
                        ThreadDecisionId::new(row.decision_id.clone()).map_err(domain_err)?,
                    )
                    .map_err(domain_err)
                })
                .transpose()?
        } else {
            None
        };
        let decisions = rows
            .into_iter()
            .map(map_thread_decision_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ThreadDecisionRevisionPage {
            decisions,
            next_cursor,
        })
    }

    async fn event_causal_context(
        &self,
        account: &SourceAccountRef,
        source_event_id: &SourceEventId,
    ) -> Result<Option<EventCausalContextView>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;

        // 事件信封（含当前档案显示名）。账号强制过滤；不存在则整体返回 None。
        let Some(event) = CausalEventRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT e.source_event_id, e.actor_platform_id, e.actor_kind,
                      e.reply_to_event_id, p.display_name
               FROM secretary_source_events e
               JOIN secretary_conversations c ON c.id = e.conversation_id
               JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
               LEFT JOIN secretary_participant_profiles p
                 ON p.account_id = e.account_id AND p.actor_platform_id = e.actor_platform_id
                    AND p.current = 1 AND p.invalidated = 0
               WHERE e.source_event_id = ? AND e.account_id = ?
                 AND c.memory_mode = 'normal' AND m.content_mode = 'normal'
                 AND NOT EXISTS (
                     SELECT 1 FROM secretary_message_tombstones tombstone
                     WHERE tombstone.source_event_id = e.source_event_id
                       AND tombstone.account_id = e.account_id
                       AND tombstone.status = 'applied'
                 )"#,
            [source_event_id.as_str().into(), account_id.into()],
        ))
        .one(&self.db)
        .await
        .map_err(store_error)?
        else {
            return Ok(None);
        };

        // 回复父事件及其发送者（同账号强制；跨账号绝不关联）。
        let reply_parent =
            load_reply_parent(&self.db, account_id, event.reply_to_event_id.as_deref()).await?;

        // 有效线程（合并/拆分后的视图）+ 根事件 + 发起人。根发送者 = 线程发起人，不是 Owner 判定。
        let thread = load_effective_thread(&self.db, account_id, source_event_id).await?;

        // @ 到的参与者（协议观察，仅 actor_id；绝不构成指派）。
        let mentioned = load_mentioned_actors(&self.db, account_id, source_event_id).await?;

        // 线程参与者有界列表（含当前档案显示名与群角色）。
        let participants = match &thread {
            Some(thread) => {
                load_thread_participants(&self.db, account, account_id, &thread.thread_id).await?
            }
            None => Vec::new(),
        };

        // 结构关系从可重建 VIEW（secretary_event_relations）读取：sent_by / mentions /
        // replies_to / member_of_thread / thread_root_by，全部账号强制。
        let mut structural =
            load_structural_relations(&self.db, account, account_id, source_event_id).await?;
        if event.reply_to_event_id.is_some() && reply_parent.is_none() {
            structural.retain(|relation| relation.kind != EventRelationKind::RepliesTo);
        }

        // 已确认要求者：线程上 status=confirmed 的 request 声明（Requester 必须带来源）。
        let confirmed_requesters =
            load_confirmed_requesters(&self.db, account_id, thread.as_ref()).await?;

        // 已确认承诺/受益方：与线程来源事件关联的 confirmed commitment 记忆。
        let confirmed_commitments =
            load_confirmed_commitments(&self.db, account_id, thread.as_ref()).await?;

        // ---- 组装类型化关系与角色列表 ----
        // 结构关系（sent_by/mentions/replies_to/member_of_thread/thread_root_by）
        // 直接采用可重建 VIEW（secretary_event_relations）的结果，不重复构造；
        // 语义角色（requested_by/promised_by/benefits）在下方从已确认来源追加。
        let mut relations: Vec<EventRelation> = structural;
        let mut source_refs: Vec<SourceEventId> = Vec::new();
        let mut push_source = |id: SourceEventId| {
            if !source_refs.iter().any(|existing| existing == &id)
                && source_refs.len() < MAX_CAUSAL_SOURCE_REFS
            {
                source_refs.push(id);
            }
        };
        push_source(source_event_id.clone());
        if let Some(ref parent) = reply_parent {
            push_source(parent.source_event_id.clone());
        }
        if let Some(ref thread) = thread {
            push_source(thread.root_event_id.clone());
        }

        let mut requesters = Vec::new();
        for claim in confirmed_requesters {
            for source in claim.source_event_ids.iter().take(MAX_RELATION_SOURCES) {
                push_source(source.clone());
            }
            let subject = account_scoped(
                account,
                claim.actor_kind.as_str(),
                claim.claimant_actor_id.as_str(),
            )?;
            relations.push(EventRelation {
                kind: EventRelationKind::RequestedBy,
                account: account.clone(),
                subject: subject.clone(),
                thread_id: thread.as_ref().map(|t| t.thread_id.clone()),
                source_event_ids: claim
                    .source_event_ids
                    .iter()
                    .take(MAX_RELATION_SOURCES)
                    .cloned()
                    .collect(),
                trust: IdentityTrust::Verified,
                confirmed: true,
                invalidation_reason: None,
            });
            requesters.push(subject);
        }

        let mut promisors = Vec::new();
        let mut beneficiaries = Vec::new();
        for commitment in confirmed_commitments {
            let sources: Vec<SourceEventId> = commitment
                .source_event_ids
                .iter()
                .take(MAX_RELATION_SOURCES)
                .cloned()
                .collect();
            for source in &sources {
                push_source(source.clone());
            }
            let promisor = account_scoped(
                account,
                commitment.promisor_kind.as_str(),
                commitment.promisor.as_str(),
            )?;
            relations.push(EventRelation {
                kind: EventRelationKind::PromisedBy,
                account: account.clone(),
                subject: promisor.clone(),
                thread_id: thread.as_ref().map(|t| t.thread_id.clone()),
                source_event_ids: sources.clone(),
                trust: IdentityTrust::Verified,
                confirmed: true,
                invalidation_reason: None,
            });
            promisors.push(promisor);
            let beneficiary = account_scoped(
                account,
                commitment.beneficiary_kind.as_str(),
                commitment.beneficiary.as_str(),
            )?;
            relations.push(EventRelation {
                kind: EventRelationKind::Benefits,
                account: account.clone(),
                subject: beneficiary.clone(),
                thread_id: thread.as_ref().map(|t| t.thread_id.clone()),
                source_event_ids: sources,
                trust: IdentityTrust::Verified,
                confirmed: true,
                invalidation_reason: None,
            });
            beneficiaries.push(beneficiary);
        }

        Ok(Some(EventCausalContextView {
            source_event_id: source_event_id.clone(),
            account: account.clone(),
            sender: Some(participant_from_parts(
                &event.actor_kind,
                &event.actor_platform_id,
                event.display_name.clone(),
            )?),
            reply_parent: reply_parent.map(|parent| CausalEventRef {
                source_event_id: parent.source_event_id.clone(),
                sender: participant_from_parts(
                    &parent.actor_kind,
                    &parent.actor_platform_id,
                    parent.display_name.clone(),
                )
                .ok(),
            }),
            thread: thread.map(|thread| CausalThreadRef {
                thread_id: thread.thread_id.clone(),
                status: thread.status,
                root_event_id: thread.root_event_id.clone(),
                root_sender: thread.root_sender.clone(),
            }),
            mentioned: mentioned
                .iter()
                .map(|actor_id| {
                    AccountScopedParticipantRef::new(
                        account.clone(),
                        PlatformIdentityKind::External,
                        actor_id.clone(),
                        IdentityTrust::Observed,
                    )
                    .map_err(domain_err)
                })
                .collect::<Result<Vec<_>, _>>()?,
            requesters,
            assignees: Vec::new(), // v1 无负责人生产者；无证据即未知，绝不猜测。
            promisors,
            beneficiaries,
            participants,
            relations,
            ambiguous: false, // 事件/参与者均为显式稳定 ID，无命名歧义。
            source_refs,
        }))
    }

    async fn participant_context(
        &self,
        account: &SourceAccountRef,
        actor_id: &str,
        conversation: Option<&ConversationRef>,
        thread_id: Option<&EventThreadId>,
    ) -> Result<Option<ParticipantContextView>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;

        // 宽松查询（调用方只有账号 + 稳定 ID）：读取全部 current 档案行并
        // fail-closed —— 恰好一行 → 以行的身份种类为准；多行（跨命名空间冲突）
        // → 显式错误，绝不静默合并；零行 → 退回事件证据的存在性判断。
        let mut profiles = load_current_profiles(&self.db, account_id, actor_id).await?;
        if profiles.len() > 1 {
            return Err(InboundEventStoreError::InvalidData(format!(
                "账号内稳定 ID {actor_id} 存在多个身份命名空间的档案，拒绝歧义读取"
            )));
        }
        let event_kind = load_actor_kind_from_events(&self.db, account_id, actor_id).await?;
        let profile = profiles.pop();
        if event_kind.is_none()
            && profile
                .as_ref()
                .is_none_or(|profile| profile.invalidated != 0)
        {
            return Ok(None);
        }
        // 身份种类优先取档案行（档案键的一部分），无档案时由最近事件恢复。
        let identity_kind = profile
            .as_ref()
            .map(|profile| profile.platform_identity_kind.clone())
            .or(event_kind)
            .unwrap_or_else(|| "external".into());
        self.participant_context_impl(
            account,
            account_id,
            &identity_kind,
            actor_id,
            conversation,
            thread_id,
            profile,
        )
        .await
    }

    /// 精确查询（调用方已知完整三元组身份）：档案按身份种类精确命中，同账号下
    /// 相同稳定 ID 的不同身份命名空间互不干扰，也不触发宽松查询的歧义拒绝。
    async fn participant_context_by_ref(
        &self,
        participant: &AccountScopedParticipantRef,
        conversation: Option<&ConversationRef>,
        thread_id: Option<&EventThreadId>,
    ) -> Result<Option<ParticipantContextView>, InboundEventStoreError> {
        let account = &participant.account;
        let account_id = resolve_account_id(&self.db, account).await?;
        let actor_id = participant.stable_id();
        let identity_kind = participant.identity.platform_kind.as_str();
        let profile =
            load_current_profile_by_kind(&self.db, account_id, identity_kind, actor_id).await?;
        let event_kind = load_actor_kind_from_events(&self.db, account_id, actor_id).await?;
        if event_kind.is_none()
            && profile
                .as_ref()
                .is_none_or(|profile| profile.invalidated != 0)
        {
            return Ok(None);
        }
        self.participant_context_impl(
            account,
            account_id,
            identity_kind,
            actor_id,
            conversation,
            thread_id,
            profile,
        )
        .await
    }

    async fn participants_by_display_name(
        &self,
        account: &SourceAccountRef,
        name: &str,
        conversation: Option<&ConversationRef>,
        thread_id: Option<&EventThreadId>,
        limit: u16,
    ) -> Result<Vec<AccountScopedParticipantRef>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        // LIKE 前缀匹配需转义通配符，避免用户输入中的 % / _ 扩展成宽匹配。
        let escaped = name
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let prefix = format!("{escaped}%");
        // 群名片只在解析出的目标会话内匹配；未提供 conversation/thread 时绝不
        // 跨所有群搜索群名片（同一名片在多个群指向不同人时不得制造歧义）。
        let observation_conversation_id = match conversation {
            Some(conversation) => {
                resolve_conversation_id(&self.db, account_id, conversation).await?
            }
            None => match thread_id {
                Some(thread_id) => {
                    resolve_thread_conversation_id(&self.db, account_id, thread_id).await?
                }
                None => None,
            },
        };

        #[derive(Debug, FromQueryResult)]
        struct CandidateRow {
            actor_platform_id: String,
            platform_identity_kind: String,
            trust: String,
        }
        // 来源有效性门与 source_refs_valid 语义一致：档案/观察的来源事件必须存在、
        // 无 applied 撤回、会话与事件均为 normal、正文投影存在，且列表非空。
        // 属性级建立来源门（P0-2）：档案显示名、观察群名片、每个别名各自携带
        // established_by_event_id / source_event_id 指向其建立事件；该事件被挤出
        // 10 条有界来源窗口或被撤回/降级后，对应属性不得再支撑 by-name 命中。
        // 单事件有效性用派生表驱动（MySQL 8.4 支持相关派生表），语义与
        // single_event_valid 完全一致：事件缺失/撤回/非 normal/无投影即失效。
        let rows = CandidateRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT p.actor_platform_id, p.platform_identity_kind, p.trust
               FROM secretary_participant_profiles p
               LEFT JOIN secretary_participant_conversation_observations o
                 ON o.account_id = p.account_id
                    AND o.platform_identity_kind = p.platform_identity_kind
                    AND o.actor_platform_id = p.actor_platform_id
                    AND o.invalidated = 0
                    -- 无会话参数（NULL）时观察不参与 JOIN，群名片绝不跨群匹配。
                    AND (? IS NOT NULL AND o.conversation_id = ?)
               WHERE p.account_id = ? AND p.current = 1 AND p.invalidated = 0
                 AND JSON_LENGTH(p.source_event_ids_json) > 0
                 AND NOT EXISTS (
                     SELECT 1 FROM JSON_TABLE(CAST(p.source_event_ids_json AS CHAR), '$[*]'
                         COLUMNS (sid VARCHAR(191) PATH '$')) jt
                     LEFT JOIN secretary_source_events e
                       ON e.source_event_id = jt.sid AND e.account_id = p.account_id
                     LEFT JOIN secretary_conversations c ON c.id = e.conversation_id
                     LEFT JOIN secretary_message_contents m ON m.source_event_id = jt.sid
                     LEFT JOIN secretary_message_tombstones t
                       ON t.source_event_id = jt.sid AND t.status = 'applied'
                     WHERE e.source_event_id IS NULL
                        OR t.source_event_id IS NOT NULL
                        OR c.memory_mode <> 'normal'
                        OR m.content_mode <> 'normal'
                        OR m.source_event_id IS NULL
                 )
                 AND (
                     -- 显示名分支：当前显示名由 established_by_event_id 建立，
                     -- 该事件必须独立有效（可能已不在有界来源列表中）。
                     (p.display_name = ? OR p.display_name LIKE ? ESCAPE '\\')
                     AND NOT EXISTS (
                         SELECT 1 FROM (SELECT p.established_by_event_id AS sid) r
                         LEFT JOIN secretary_source_events ed
                           ON ed.source_event_id = r.sid AND ed.account_id = p.account_id
                         LEFT JOIN secretary_conversations cd ON cd.id = ed.conversation_id
                         LEFT JOIN secretary_message_contents md ON md.source_event_id = r.sid
                         LEFT JOIN secretary_message_tombstones td
                           ON td.source_event_id = r.sid AND td.status = 'applied'
                         WHERE ed.source_event_id IS NULL
                            OR td.source_event_id IS NOT NULL
                            OR cd.memory_mode <> 'normal'
                            OR md.content_mode <> 'normal'
                            OR md.source_event_id IS NULL
                     )
                     -- 别名分支：命中的别名必须携带自身来源事件且该来源独立有效，
                     -- 不依赖聚合来源列表（旧显示名建立事件可能已被淘汰）。
                     OR EXISTS (SELECT 1 FROM JSON_TABLE(CAST(p.aliases_json AS CHAR), '$[*]'
                         COLUMNS (alias VARCHAR(200) PATH '$.alias',
                                  src VARCHAR(191) PATH '$.source_event_id')) aj
                         WHERE (aj.alias = ? OR aj.alias LIKE ? ESCAPE '\\')
                           AND NOT EXISTS (
                               SELECT 1 FROM (SELECT aj.src AS sid) r2
                               LEFT JOIN secretary_source_events ea
                                 ON ea.source_event_id = r2.sid AND ea.account_id = p.account_id
                               LEFT JOIN secretary_conversations ca ON ca.id = ea.conversation_id
                               LEFT JOIN secretary_message_contents ma ON ma.source_event_id = r2.sid
                               LEFT JOIN secretary_message_tombstones ta
                                 ON ta.source_event_id = r2.sid AND ta.status = 'applied'
                               WHERE ea.source_event_id IS NULL
                                  OR ta.source_event_id IS NOT NULL
                                  OR ca.memory_mode <> 'normal'
                                  OR ma.content_mode <> 'normal'
                                  OR ma.source_event_id IS NULL
                           ))
                     OR (o.observation_id IS NOT NULL
                         AND JSON_LENGTH(o.source_event_ids_json) > 0
                         AND (o.group_card = ? OR o.group_card LIKE ? ESCAPE '\\')
                         -- 群名片分支：当前名片由 o.established_by_event_id 建立，
                         -- 同样要求建立事件独立有效。
                         AND NOT EXISTS (
                             SELECT 1 FROM (SELECT o.established_by_event_id AS sid) r3
                             LEFT JOIN secretary_source_events eo
                               ON eo.source_event_id = r3.sid AND eo.account_id = o.account_id
                             LEFT JOIN secretary_conversations co ON co.id = eo.conversation_id
                             LEFT JOIN secretary_message_contents mo ON mo.source_event_id = r3.sid
                             LEFT JOIN secretary_message_tombstones tz
                               ON tz.source_event_id = r3.sid AND tz.status = 'applied'
                             WHERE eo.source_event_id IS NULL
                                OR tz.source_event_id IS NOT NULL
                                OR co.memory_mode <> 'normal'
                                OR mo.content_mode <> 'normal'
                                OR mo.source_event_id IS NULL
                         )
                         AND NOT EXISTS (
                             SELECT 1 FROM JSON_TABLE(CAST(o.source_event_ids_json AS CHAR), '$[*]'
                                 COLUMNS (sid VARCHAR(191) PATH '$')) jo
                             LEFT JOIN secretary_source_events e2
                               ON e2.source_event_id = jo.sid AND e2.account_id = o.account_id
                             LEFT JOIN secretary_conversations c2 ON c2.id = e2.conversation_id
                             LEFT JOIN secretary_message_contents m2 ON m2.source_event_id = jo.sid
                             LEFT JOIN secretary_message_tombstones t2
                               ON t2.source_event_id = jo.sid AND t2.status = 'applied'
                             WHERE e2.source_event_id IS NULL
                                OR t2.source_event_id IS NOT NULL
                                OR c2.memory_mode <> 'normal'
                                OR m2.content_mode <> 'normal'
                                OR m2.source_event_id IS NULL
                         ))
                 )
               GROUP BY p.actor_platform_id, p.platform_identity_kind, p.trust
               ORDER BY MAX(p.updated_at) DESC
               LIMIT ?"#,
            [
                // NULL 参数用 Option 内的 None（sea-query 1.x Value 无 Null 变体）。
                observation_conversation_id
                    .map(sea_orm::Value::from)
                    .unwrap_or(sea_orm::Value::BigUnsigned(None)),
                observation_conversation_id
                    .map(sea_orm::Value::from)
                    .unwrap_or(sea_orm::Value::BigUnsigned(None)),
                account_id.into(),
                name.into(),
                prefix.clone().into(),
                name.into(),
                prefix.clone().into(),
                name.into(),
                prefix.into(),
                (limit as i64).into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        rows.into_iter()
            .map(|row| {
                let kind = parse_actor_kind(&row.platform_identity_kind)?;
                let trust = parse_identity_trust(&row.trust)?;
                AccountScopedParticipantRef::new(
                    account.clone(),
                    PlatformIdentityKind::from_verified_actor_kind(kind),
                    row.actor_platform_id,
                    trust,
                )
                .map_err(domain_err)
            })
            .collect()
    }

    /// 列出账号最近的 N 条事件证据视图，包含发送者、@、Reply、Thread 和内容策略。
    /// 数据库先按 received_at 倒序取最近 N 条，Rust 侧再反转为时间正序。
    async fn list_recent_event_views(
        &self,
        account: &SourceAccountRef,
        limit: u16,
        visibility: RetrievalVisibility,
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
               AND (
                   (c.memory_mode = 'normal' AND m.content_mode = 'normal')
                   OR (? AND c.memory_mode IN ('normal', 'local_only')
                           AND m.content_mode IN ('normal', 'local_only'))
               )
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
            vec![
                account_id.into(),
                visibility.includes_local_only().into(),
                (limit as u64).into(),
            ],
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

    async fn list_projects(
        &self,
        account: &SourceAccountRef,
        limit: u16,
    ) -> Result<Vec<crate::ProjectMemorySummary>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        let now = Utc::now().timestamp();
        let rows = ProjectListRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT f.fact_id, CAST(f.fact_json AS CHAR) AS fact_json,
                      CAST(UNIX_TIMESTAMP(f.updated_at) AS SIGNED) AS updated_at_unix
               FROM secretary_memory_facts f
               WHERE f.account_id = ? AND f.fact_kind = 'project'
                 AND f.fact_status = 'confirmed'
                 AND (f.valid_until_unix_secs IS NULL OR f.valid_until_unix_secs > ?)
                 AND EXISTS (
                   SELECT 1 FROM secretary_memory_fact_sources fs0
                   WHERE fs0.fact_id = f.fact_id
                 )
               AND NOT EXISTS (
                   SELECT 1 FROM secretary_memory_fact_sources fs
                   LEFT JOIN secretary_source_events se
                     ON se.source_event_id = fs.source_event_id AND se.account_id = ?
                   LEFT JOIN secretary_message_tombstones t
                     ON t.source_event_id = fs.source_event_id AND t.status = 'applied'
                   LEFT JOIN secretary_conversations c ON c.id = se.conversation_id
                   LEFT JOIN secretary_message_contents mc ON mc.source_event_id = fs.source_event_id
                   WHERE fs.fact_id = f.fact_id
                     AND (se.source_event_id IS NULL
                          OR t.source_event_id IS NOT NULL
                          OR c.memory_mode IS NULL OR c.memory_mode <> 'normal'
                          OR mc.content_mode IS NULL OR mc.content_mode <> 'normal'
                          OR mc.source_event_id IS NULL)
                 )
               ORDER BY f.updated_at DESC, f.fact_id DESC
               LIMIT ?"#,
            vec![
                account_id.into(),
                now.into(),
                account_id.into(),
                (limit as u64).into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        rows.into_iter()
            .map(|row| {
                let fact: crate::MemoryFact =
                    serde_json::from_str(&row.fact_json).map_err(|e| {
                        InboundEventStoreError::InvalidData(format!("invalid project fact: {e}"))
                    })?;
                let project = match &fact.payload {
                    crate::MemoryPayload::Project(p) => p,
                    _ => {
                        return Err(InboundEventStoreError::InvalidData(
                            "stored fact is not a project".into(),
                        ));
                    }
                };
                let pk = project.project_key.clone();
                let goal = project.goal.chars().take(200).collect::<String>();
                let member_count = project.effective_members().len();
                let progress = project
                    .progress
                    .as_ref()
                    .map(|p| p.chars().take(200).collect::<String>());
                let risk_count = project.risks.len();
                let blocker_count = project.blockers.len();
                Ok(crate::ProjectMemorySummary {
                    project_key: pk,
                    goal,
                    member_count,
                    progress,
                    risk_count,
                    blocker_count,
                    fact_id: crate::MemoryFactId::new(row.fact_id)
                        .map_err(|e| InboundEventStoreError::InvalidData(e.to_string()))?,
                    updated_at_unix_secs: Some(row.updated_at_unix),
                })
            })
            .collect()
    }

    async fn query_project(
        &self,
        account: &SourceAccountRef,
        project_key: &str,
    ) -> Result<Option<crate::ProjectContextView>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, account).await?;
        let now = Utc::now().timestamp();
        let row = ProjectDetailRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT f.fact_id, CAST(f.fact_json AS CHAR) AS fact_json,
                      f.confidence_bps, f.valid_until_unix_secs
               FROM secretary_memory_facts f
               WHERE f.account_id = ? AND f.fact_kind = 'project'
                 AND f.fact_status = 'confirmed'
                 AND (f.valid_until_unix_secs IS NULL OR f.valid_until_unix_secs > ?)
                 AND JSON_UNQUOTE(JSON_EXTRACT(f.fact_json, '$.payload.data.project_key')) = ?
               ORDER BY f.updated_at DESC, f.fact_id DESC
               LIMIT 1"#,
            vec![account_id.into(), now.into(), project_key.into()],
        ))
        .one(&self.db)
        .await
        .map_err(store_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let fact: crate::MemoryFact = serde_json::from_str(&row.fact_json).map_err(|e| {
            InboundEventStoreError::InvalidData(format!("invalid project fact: {e}"))
        })?;
        let project = match &fact.payload {
            crate::MemoryPayload::Project(p) => p,
            _ => {
                return Err(InboundEventStoreError::InvalidData(
                    "stored fact is not a project".into(),
                ));
            }
        };
        // 验证所有来源均仍有效（fail-closed：任一无效或零来源即拒绝）。
        let source_valid =
            ProjectSourceCheckRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"SELECT 1 AS present
               FROM secretary_memory_fact_sources fs0
               WHERE fs0.fact_id = ?
                 AND NOT EXISTS (
                   SELECT 1 FROM secretary_memory_fact_sources fs
                   LEFT JOIN secretary_source_events se
                     ON se.source_event_id = fs.source_event_id AND se.account_id = ?
                   LEFT JOIN secretary_message_tombstones t
                     ON t.source_event_id = fs.source_event_id AND t.status = 'applied'
                   LEFT JOIN secretary_conversations c ON c.id = se.conversation_id
                   LEFT JOIN secretary_message_contents mc
                     ON mc.source_event_id = fs.source_event_id
                   WHERE fs.fact_id = fs0.fact_id
                     AND (se.source_event_id IS NULL
                          OR t.source_event_id IS NOT NULL
                          OR c.memory_mode IS NULL OR c.memory_mode <> 'normal'
                          OR mc.content_mode IS NULL OR mc.content_mode <> 'normal'
                          OR mc.source_event_id IS NULL)
                 )
               LIMIT 1"#,
                vec![row.fact_id.clone().into(), account_id.into()],
            ))
            .one(&self.db)
            .await
            .map_err(store_error)?;
        if source_valid.is_none() {
            return Ok(None);
        }
        // Clone project for owned values (behind &fact.payload reference).
        let project_key = project.project_key.clone();
        let goal = project.goal.clone();
        let has_member_refs = !project.member_actor_refs.is_empty();
        let legacy_member_ids = !has_member_refs && !project.member_actor_ids.is_empty();
        let members = if has_member_refs {
            project.member_actor_refs.clone()
        } else {
            project
                .member_actor_ids
                .iter()
                .map(|id| crate::ProjectMemberRef {
                    platform_identity_kind: None,
                    actor_id: id.clone(),
                })
                .collect()
        };
        let decisions: Vec<crate::ThreadDecisionId> = project.decision_ids.clone();
        let progress = project.progress.clone();
        let risks = project.risks.clone();
        let blockers = project.blockers.clone();
        let artifact_refs = project.artifact_refs.clone();
        Ok(Some(crate::ProjectContextView {
            project_key,
            goal,
            members,
            legacy_member_ids,
            progress,
            risks,
            blockers,
            artifact_refs,
            decision_ids: decisions,
            fact_id: crate::MemoryFactId::new(row.fact_id)
                .map_err(|e| InboundEventStoreError::InvalidData(e.to_string()))?,
            confidence_bps: row.confidence_bps,
            source_event_ids: fact.source_event_ids,
            valid_until_unix_secs: row.valid_until_unix_secs,
        }))
    }

    async fn list_commitments(
        &self,
        query: &crate::CommitmentQuery,
    ) -> Result<Vec<crate::CommitmentSummary>, InboundEventStoreError> {
        let account_id = resolve_account_id(&self.db, &query.account).await?;
        let now = Utc::now().timestamp();
        let limit = query.limit.clamp(1, 100);
        let mut params: Vec<sea_orm::Value> = Vec::new();
        params.push(account_id.into());
        params.push(now.into());
        params.push(account_id.into());
        // 动态拼接状态过滤
        let status_clause = match &query.status {
            Some(status) => {
                params.push(status.as_str().into());
                " AND JSON_UNQUOTE(JSON_EXTRACT(f.fact_json, '$.payload.data.status')) = ?"
            }
            None => "",
        };
        // 动态拼接承诺人过滤（kind + actor_id，fail-closed）
        let promisor_clause = match &query.promisor {
            Some(promisor) => {
                params.push(promisor.actor_id.clone().into());
                if let Some(kind) = promisor.platform_identity_kind {
                    params.push(kind.serialized_name().into());
                    " AND JSON_UNQUOTE(JSON_EXTRACT(f.fact_json, '$.payload.data.promisor.actor_id')) = ? AND JSON_UNQUOTE(JSON_EXTRACT(f.fact_json, '$.payload.data.promisor.platform_identity_kind')) = ?"
                } else {
                    // 旧数据无 kind 字段：要求 IS NULL（有 kind 的新数据不会错误命中）。
                    " AND JSON_UNQUOTE(JSON_EXTRACT(f.fact_json, '$.payload.data.promisor.actor_id')) = ? AND JSON_EXTRACT(f.fact_json, '$.payload.data.promisor.platform_identity_kind') IS NULL"
                }
            }
            None => "",
        };
        // 动态拼接受益方过滤（kind + actor_id，fail-closed）
        let beneficiary_clause = match &query.beneficiary {
            Some(beneficiary) => {
                params.push(beneficiary.actor_id.clone().into());
                if let Some(kind) = beneficiary.platform_identity_kind {
                    params.push(kind.serialized_name().into());
                    " AND JSON_UNQUOTE(JSON_EXTRACT(f.fact_json, '$.payload.data.beneficiary.actor_id')) = ? AND JSON_UNQUOTE(JSON_EXTRACT(f.fact_json, '$.payload.data.beneficiary.platform_identity_kind')) = ?"
                } else {
                    " AND JSON_UNQUOTE(JSON_EXTRACT(f.fact_json, '$.payload.data.beneficiary.actor_id')) = ? AND JSON_EXTRACT(f.fact_json, '$.payload.data.beneficiary.platform_identity_kind') IS NULL"
                }
            }
            None => "",
        };
        // 动态拼接 due 范围
        let due_since_clause = match query.due_since_unix_secs {
            Some(since) => {
                params.push(since.into());
                " AND JSON_EXTRACT(f.fact_json, '$.payload.data.due_at_unix_secs') >= ?"
            }
            None => "",
        };
        let due_until_clause = match query.due_until_unix_secs {
            Some(until) => {
                params.push(until.into());
                " AND JSON_EXTRACT(f.fact_json, '$.payload.data.due_at_unix_secs') <= ?"
            }
            None => "",
        };
        params.push((limit as u64).into());
        let sql = format!(
            r#"SELECT f.fact_id, CAST(f.fact_json AS CHAR) AS fact_json,
                      fu.follow_up_id, fu.status AS follow_up_status
               FROM secretary_memory_facts f
               LEFT JOIN secretary_follow_up_items fu
                 ON fu.source_memory_fact_id = f.fact_id
                 AND fu.status = 'scheduled'
               WHERE f.account_id = ? AND f.fact_kind = 'commitment'
                 AND f.fact_status = 'confirmed'
                 AND (f.valid_until_unix_secs IS NULL OR f.valid_until_unix_secs > ?)
                 AND EXISTS (
                   SELECT 1 FROM secretary_memory_fact_sources fs0
                   WHERE fs0.fact_id = f.fact_id
                 )
                 AND NOT EXISTS (
                   SELECT 1 FROM secretary_memory_fact_sources fs
                   LEFT JOIN secretary_source_events se
                     ON se.source_event_id = fs.source_event_id AND se.account_id = ?
                   LEFT JOIN secretary_message_tombstones t
                     ON t.source_event_id = fs.source_event_id AND t.status = 'applied'
                   LEFT JOIN secretary_conversations c ON c.id = se.conversation_id
                   LEFT JOIN secretary_message_contents mc
                     ON mc.source_event_id = fs.source_event_id
                   WHERE fs.fact_id = f.fact_id
                     AND (se.source_event_id IS NULL
                          OR t.source_event_id IS NOT NULL
                          OR c.memory_mode IS NULL OR c.memory_mode <> 'normal'
                          OR mc.content_mode IS NULL OR mc.content_mode <> 'normal'
                          OR mc.source_event_id IS NULL)
                 ){status_clause}{promisor_clause}{beneficiary_clause}{due_since_clause}{due_until_clause}
               ORDER BY f.updated_at DESC, f.fact_id DESC
               LIMIT ?"#
        );
        let rows = CommitmentListRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            &sql,
            params,
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        rows.into_iter()
            .map(|row| {
                let fact: crate::MemoryFact =
                    serde_json::from_str(&row.fact_json).map_err(|e| {
                        InboundEventStoreError::InvalidData(format!("invalid commitment fact: {e}"))
                    })?;
                let commitment = match &fact.payload {
                    crate::MemoryPayload::Commitment(c) => c,
                    _ => {
                        return Err(InboundEventStoreError::InvalidData(
                            "stored fact is not a commitment".into(),
                        ));
                    }
                };
                Ok(crate::CommitmentSummary {
                    fact_id: crate::MemoryFactId::new(row.fact_id)
                        .map_err(|e| InboundEventStoreError::InvalidData(e.to_string()))?,
                    promisor: crate::ProjectMemberRef {
                        platform_identity_kind: commitment.promisor.platform_identity_kind,
                        actor_id: commitment.promisor.actor_id.clone(),
                    },
                    beneficiary: crate::ProjectMemberRef {
                        platform_identity_kind: commitment.beneficiary.platform_identity_kind,
                        actor_id: commitment.beneficiary.actor_id.clone(),
                    },
                    action: commitment.action.clone(),
                    due_at_unix_secs: commitment.due_at_unix_secs,
                    status: commitment.status,
                    source_event_ids: fact.source_event_ids.clone(),
                    follow_up_id: row.follow_up_id,
                    follow_up_status: row.follow_up_status,
                })
            })
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

/// 转义 LIKE 模式中的通配符（% / _）与转义符本身（CMD-009 目标 B）。
/// Owner 提供的查询文本绝不能改变匹配范围：`%`、`_` 必须按字面匹配，
/// 反斜杠必须先于通配符转义（否则 `\%` 会变成转义后的 `%` 字面量）。
fn escape_like_pattern(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '%' => escaped.push_str("\\%"),
            '_' => escaped.push_str("\\_"),
            other => escaped.push(other),
        }
    }
    escaped
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
        representative_source_event_id: SourceEventId::new(&row.representative_source_event_id)
            .map_err(domain_err)?,
        representative_conversation: ConversationRef::new(
            parse_conversation_kind(&row.conversation_kind)?,
            &row.platform_conversation_id,
        )
        .map_err(domain_err)?,
        representative_actor: VerifiedActor::new(
            parse_actor_kind(&row.actor_kind)?,
            &row.actor_platform_id,
        )
        .map_err(domain_err)?,
        representative_occurred_at_unix_secs: row.representative_occurred_at,
        representative_excerpt: row.representative_excerpt.unwrap_or_default(),
        representative_content_trust_level: parse_memory_mode(&row.memory_mode)?,
        match_rank: match row.match_rank {
            3 => crate::ThreadSearchMatchRank::Exact,
            2 => crate::ThreadSearchMatchRank::Prefix,
            1 => crate::ThreadSearchMatchRank::Contains,
            other => {
                return Err(InboundEventStoreError::InvalidData(format!(
                    "unknown thread search match rank: {other}"
                )));
            }
        },
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
    // 事件派生的 Actor 引用携带身份种类（来自事件 actor_kind），
    // 使 TempRefMap 能映射为完整账号作用域参与者引用。
    let actor = ThreadActorRef {
        account: account.clone(),
        actor_id: row.actor_platform_id,
        platform_identity_kind: Some(
            parse_actor_kind(&row.actor_kind)
                .map(PlatformIdentityKind::from_verified_actor_kind)?,
        ),
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
            // @ 到的参与者由协议只带 actor_id；事件关系 VIEW 中 mentions 固定为
            // external 观察（提及 ≠ 指派），身份种类按 External 处理。
            platform_identity_kind: Some(PlatformIdentityKind::External),
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
    let confidence_bps = u16::try_from(row.confidence_bps).map_err(|_| {
        InboundEventStoreError::InvalidData(
            "database returned an out-of-range decision confidence".into(),
        )
    })?;
    if confidence_bps > 10_000 {
        return Err(InboundEventStoreError::InvalidData(
            "database returned an out-of-range decision confidence".into(),
        ));
    }
    if row.created_at_unix_micros < 0 {
        return Err(InboundEventStoreError::InvalidData(
            "database returned a decision timestamp before the Unix epoch".into(),
        ));
    }
    Ok(ThreadDecisionSummary {
        decision_id: ThreadDecisionId::new(row.decision_id).map_err(domain_err)?,
        status: row.status,
        statement: row.statement,
        confidence_bps,
        supersedes: row
            .supersedes_id
            .map(ThreadDecisionId::new)
            .transpose()
            .map_err(domain_err)?,
        created_at_unix_micros: row.created_at_unix_micros,
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
    confidence_bps: u32,
    supersedes_id: Option<String>,
    created_at_unix_micros: i64,
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
    representative_source_event_id: String,
    actor_platform_id: String,
    actor_kind: String,
    representative_occurred_at: i64,
    platform_conversation_id: String,
    conversation_kind: String,
    memory_mode: String,
    representative_excerpt: Option<String>,
    match_rank: i32,
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

// ===== 事件因果上下文与参与者上下文的查询辅助（THR-011/THR-012/ID-004/ID-005）=====

/// 事件发送者身份的可信等级：Owner 来自绑定判定（Verified），其余为协议观察（Observed）。
fn identity_trust_for_kind(actor_kind: &str) -> IdentityTrust {
    if actor_kind == "owner" {
        IdentityTrust::Verified
    } else {
        IdentityTrust::Observed
    }
}

/// 构造账号作用域参与者引用（复用 ParticipantIdentity，不建平行身份体系）。
fn account_scoped(
    account: &SourceAccountRef,
    actor_kind: &str,
    actor_platform_id: &str,
) -> Result<AccountScopedParticipantRef, InboundEventStoreError> {
    let kind = parse_actor_kind(actor_kind)?;
    AccountScopedParticipantRef::new(
        account.clone(),
        PlatformIdentityKind::from_verified_actor_kind(kind),
        actor_platform_id,
        identity_trust_for_kind(actor_kind),
    )
    .map_err(domain_err)
}

/// 构造带显示名的 ParticipantIdentity（显示名只做展示与指代候选，不授权）。
fn participant_from_parts(
    actor_kind: &str,
    actor_platform_id: &str,
    display_name: Option<String>,
) -> Result<ParticipantIdentity, InboundEventStoreError> {
    let mut identity = participant_for(actor_kind, actor_platform_id)?;
    identity.display_name = display_name.filter(|name| !name.trim().is_empty());
    Ok(identity)
}

fn parse_identity_trust(value: &str) -> Result<IdentityTrust, InboundEventStoreError> {
    match value {
        "verified" => Ok(IdentityTrust::Verified),
        "observed" => Ok(IdentityTrust::Observed),
        "inferred" => Ok(IdentityTrust::Inferred),
        other => Err(InboundEventStoreError::InvalidData(format!(
            "unknown participant profile trust: {other}"
        ))),
    }
}

/// 解析档案 aliases_json（[{"alias","source_event_id"}] → 有界字符串列表）。
fn parse_aliases(value: &str) -> Result<Vec<String>, InboundEventStoreError> {
    if value.trim().is_empty() || value == "null" {
        return Ok(Vec::new());
    }
    let items: Vec<serde_json::Value> = serde_json::from_str(value).map_err(|error| {
        InboundEventStoreError::InvalidData(format!(
            "participant profile aliases_json decode failed: {error}"
        ))
    })?;
    Ok(items
        .into_iter()
        .filter_map(|item| {
            item.get("alias")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect())
}

#[derive(Debug, FromQueryResult)]
struct CausalEventRow {
    source_event_id: String,
    actor_platform_id: String,
    actor_kind: String,
    reply_to_event_id: Option<String>,
    display_name: Option<String>,
}

/// 回复父事件的类型化数据（同账号强制；跨账号绝不关联）。
struct CausalParentData {
    source_event_id: SourceEventId,
    actor_platform_id: String,
    actor_kind: String,
    display_name: Option<String>,
}

/// 回复父事件（同账号强制；跨账号绝不关联）。
async fn load_reply_parent(
    db: &DatabaseConnection,
    account_id: u64,
    reply_to_event_id: Option<&str>,
) -> Result<Option<CausalParentData>, InboundEventStoreError> {
    let Some(parent_id) = reply_to_event_id else {
        return Ok(None);
    };
    let row = CausalEventRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT p.source_event_id, p.actor_platform_id, p.actor_kind,
                  p.reply_to_event_id, pp.display_name
           FROM secretary_source_events p
           JOIN secretary_conversations c ON c.id = p.conversation_id
           JOIN secretary_message_contents m ON m.source_event_id = p.source_event_id
           LEFT JOIN secretary_participant_profiles pp
             ON pp.account_id = p.account_id AND pp.actor_platform_id = p.actor_platform_id
                AND pp.current = 1 AND pp.invalidated = 0
           WHERE p.source_event_id = ? AND p.account_id = ?
             AND c.memory_mode = 'normal' AND m.content_mode = 'normal'
             AND NOT EXISTS (
                 SELECT 1 FROM secretary_message_tombstones tombstone
                 WHERE tombstone.source_event_id = p.source_event_id
                   AND tombstone.account_id = p.account_id
                   AND tombstone.status = 'applied'
             )"#,
        [parent_id.into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(store_error)?;
    row.map(|row| {
        Ok(CausalParentData {
            source_event_id: SourceEventId::new(row.source_event_id).map_err(domain_err)?,
            actor_platform_id: row.actor_platform_id,
            actor_kind: row.actor_kind,
            display_name: row.display_name,
        })
    })
    .transpose()
}

#[derive(Debug, FromQueryResult)]
struct CausalThreadRow {
    thread_id: String,
    status: String,
    root_event_id: String,
    root_actor_platform_id: String,
    root_actor_kind: String,
    root_display_name: Option<String>,
}

/// 有效线程的类型化数据（根发送者 = 线程发起人，不是 Owner 判定）。
struct CausalThreadData {
    thread_id: EventThreadId,
    status: ThreadStatus,
    root_event_id: SourceEventId,
    root_sender: Option<ParticipantIdentity>,
}

/// 有效线程（合并/拆分后的 secretary_effective_thread_events 视图）+ 根事件 + 发起人。
async fn load_effective_thread(
    db: &DatabaseConnection,
    account_id: u64,
    source_event_id: &SourceEventId,
) -> Result<Option<CausalThreadData>, InboundEventStoreError> {
    let row = CausalThreadRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT t.thread_id, t.status, t.root_event_id,
                  r.actor_platform_id AS root_actor_platform_id,
                  r.actor_kind AS root_actor_kind,
                  rp.display_name AS root_display_name
           FROM secretary_effective_thread_events ev
           JOIN secretary_event_threads t ON t.thread_id = ev.thread_id
           JOIN secretary_source_events e ON e.source_event_id = ev.source_event_id
           JOIN secretary_source_events r ON r.source_event_id = t.root_event_id
           JOIN secretary_conversations rc ON rc.id = r.conversation_id
           JOIN secretary_message_contents rm ON rm.source_event_id = r.source_event_id
           LEFT JOIN secretary_participant_profiles rp
             ON rp.account_id = t.account_id AND rp.actor_platform_id = r.actor_platform_id
                AND rp.current = 1 AND rp.invalidated = 0
           WHERE ev.source_event_id = ? AND e.account_id = ? AND t.account_id = ?
             AND rc.memory_mode = 'normal' AND rm.content_mode = 'normal'
             AND NOT EXISTS (
                 SELECT 1 FROM secretary_message_tombstones tombstone
                 WHERE tombstone.source_event_id = r.source_event_id
                   AND tombstone.account_id = r.account_id
                   AND tombstone.status = 'applied'
             )
           LIMIT 1"#,
        [
            source_event_id.as_str().into(),
            account_id.into(),
            account_id.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(store_error)?;
    row.map(|row| {
        Ok(CausalThreadData {
            thread_id: EventThreadId::new(row.thread_id).map_err(domain_err)?,
            status: parse_thread_status(&row.status)?,
            root_event_id: SourceEventId::new(row.root_event_id).map_err(domain_err)?,
            root_sender: Some(participant_from_parts(
                &row.root_actor_kind,
                &row.root_actor_platform_id,
                row.root_display_name,
            )?),
        })
    })
    .transpose()
}

/// @ 到的参与者（协议观察，仅 actor_id；绝不构成指派）。
async fn load_mentioned_actors(
    db: &DatabaseConnection,
    account_id: u64,
    source_event_id: &SourceEventId,
) -> Result<Vec<String>, InboundEventStoreError> {
    #[derive(Debug, FromQueryResult)]
    struct MentionedRow {
        mentioned_json: Option<String>,
    }
    let row = MentionedRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT CAST(m.mentioned_actor_ids AS CHAR) AS mentioned_json
           FROM secretary_message_contents m
           JOIN secretary_source_events e ON e.source_event_id = m.source_event_id
           WHERE m.source_event_id = ? AND e.account_id = ?"#,
        [source_event_id.as_str().into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(store_error)?;
    let Some(row) = row else {
        return Ok(Vec::new());
    };
    let Some(json) = row.mentioned_json else {
        return Ok(Vec::new());
    };
    if json.trim().is_empty() || json == "null" {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = serde_json::from_str(&json).map_err(|error| {
        InboundEventStoreError::InvalidData(format!("invalid mentioned_actor_ids JSON: {error}"))
    })?;
    Ok(ids.into_iter().take(MAX_CAUSAL_MENTIONED).collect())
}

#[derive(Debug, FromQueryResult)]
struct ThreadParticipantRow {
    actor_platform_id: String,
    actor_kind: String,
    event_count: i64,
    display_name: Option<String>,
    group_role: Option<String>,
}

/// 线程参与者有界列表（含当前档案显示名与事件所在会话的群角色，不含正文）。
/// 线程成员关系取自 `secretary_effective_thread_events`（合并/拆分后的有效线程，
/// 不返回旧 Thread 成员）；群角色取自会话作用域观察，绝不跨会话猜测。
async fn load_thread_participants(
    db: &DatabaseConnection,
    account: &SourceAccountRef,
    account_id: u64,
    thread_id: &EventThreadId,
) -> Result<Vec<EventParticipantSummary>, InboundEventStoreError> {
    let rows = ThreadParticipantRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT e.actor_platform_id, e.actor_kind, COUNT(*) AS event_count,
                  p.display_name, o.group_role
           FROM secretary_effective_thread_events te
           JOIN secretary_source_events e ON e.source_event_id = te.source_event_id
           JOIN secretary_conversations c ON c.id = e.conversation_id
           JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
           LEFT JOIN secretary_participant_profiles p
             ON p.account_id = e.account_id
                AND p.platform_identity_kind = e.actor_kind
                AND p.actor_platform_id = e.actor_platform_id
                AND p.current = 1 AND p.invalidated = 0
           LEFT JOIN secretary_participant_conversation_observations o
             ON o.account_id = e.account_id
                AND o.conversation_id = e.conversation_id
                AND o.platform_identity_kind = e.actor_kind
                AND o.actor_platform_id = e.actor_platform_id
                AND o.invalidated = 0
           WHERE te.thread_id = ? AND e.account_id = ?
             AND c.memory_mode = 'normal' AND m.content_mode = 'normal'
             AND NOT EXISTS (
                 SELECT 1 FROM secretary_message_tombstones tombstone
                 WHERE tombstone.source_event_id = e.source_event_id
                   AND tombstone.account_id = e.account_id
                   AND tombstone.status = 'applied'
             )
           GROUP BY e.actor_platform_id, e.actor_kind, p.display_name, o.group_role
           ORDER BY event_count DESC, e.actor_platform_id
           LIMIT ?"#,
        [
            thread_id.as_str().into(),
            account_id.into(),
            (MAX_CAUSAL_PARTICIPANTS as i64).into(),
        ],
    ))
    .all(db)
    .await
    .map_err(store_error)?;
    rows.into_iter()
        .map(|row| {
            let kind = parse_actor_kind(&row.actor_kind)?;
            let participant = AccountScopedParticipantRef::new(
                account.clone(),
                PlatformIdentityKind::from_verified_actor_kind(kind),
                row.actor_platform_id,
                identity_trust_for_kind(&row.actor_kind),
            )
            .map_err(domain_err)?;
            Ok(EventParticipantSummary {
                participant,
                display_name: row.display_name.filter(|name| !name.trim().is_empty()),
                group_role: GroupRole::parse_protocol(row.group_role.as_deref()),
                event_count: checked_count(row.event_count)?,
            })
        })
        .collect()
}

#[derive(Debug, FromQueryResult)]
struct StructuralRelationRow {
    relation_kind: String,
    subject_actor_id: String,
    subject_actor_kind: String,
    thread_id: Option<String>,
    confirmed: i8,
}

/// 结构关系从可重建 VIEW（secretary_event_relations）读取，账号强制过滤。
async fn load_structural_relations(
    db: &DatabaseConnection,
    account: &SourceAccountRef,
    account_id: u64,
    source_event_id: &SourceEventId,
) -> Result<Vec<EventRelation>, InboundEventStoreError> {
    let rows = StructuralRelationRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT relation_kind, subject_actor_id, subject_actor_kind, thread_id, confirmed
           FROM secretary_event_relations
           WHERE source_event_id = ? AND account_id = ?
           ORDER BY relation_kind, subject_actor_id"#,
        [source_event_id.as_str().into(), account_id.into()],
    ))
    .all(db)
    .await
    .map_err(store_error)?;
    rows.into_iter()
        .map(|row| {
            let kind = EventRelationKind::parse(&row.relation_kind).ok_or_else(|| {
                InboundEventStoreError::InvalidData(format!(
                    "unknown relation_kind in view: {}",
                    row.relation_kind
                ))
            })?;
            Ok(EventRelation {
                kind,
                account: account.clone(),
                subject: AccountScopedParticipantRef::new(
                    account.clone(),
                    PlatformIdentityKind::from_verified_actor_kind(parse_actor_kind(
                        &row.subject_actor_kind,
                    )?),
                    row.subject_actor_id,
                    identity_trust_for_kind(&row.subject_actor_kind),
                )
                .map_err(domain_err)?,
                thread_id: row
                    .thread_id
                    .map(EventThreadId::new)
                    .transpose()
                    .map_err(domain_err)?,
                source_event_ids: vec![source_event_id.clone()],
                trust: identity_trust_for_kind(&row.subject_actor_kind),
                confirmed: row.confirmed != 0,
                invalidation_reason: None,
            })
        })
        .collect()
}

#[derive(Debug, FromQueryResult)]
struct ConfirmedRequesterRow {
    claimant_actor_id: String,
    actor_kind: String,
    source_event_ids: Option<String>,
}

/// 已确认要求者的类型化数据（来源可回读）。
struct ConfirmedRequesterData {
    claimant_actor_id: String,
    actor_kind: String,
    source_event_ids: Vec<SourceEventId>,
}

/// 已确认要求者：线程上 status=confirmed 的 request 声明，来源可回读。
async fn load_confirmed_requesters(
    db: &DatabaseConnection,
    account_id: u64,
    thread: Option<&CausalThreadData>,
) -> Result<Vec<ConfirmedRequesterData>, InboundEventStoreError> {
    let Some(thread) = thread else {
        return Ok(Vec::new());
    };
    let rows = ConfirmedRequesterRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT c.claimant_actor_id, c.claim_kind, c.status,
                  GROUP_CONCAT(s.source_event_id SEPARATOR ',') AS source_event_ids,
                  (SELECT e2.actor_kind
                   FROM secretary_source_events e2
                   WHERE e2.account_id = t.account_id
                     AND e2.actor_platform_id = c.claimant_actor_id
                   ORDER BY e2.occurred_at_unix_secs DESC, e2.source_event_id DESC
                   LIMIT 1) AS actor_kind
           FROM secretary_thread_claims c
           JOIN secretary_event_threads t ON t.thread_id = c.thread_id
           JOIN secretary_thread_claim_sources s ON s.claim_id = c.claim_id
           WHERE c.thread_id = ? AND t.account_id = ?
             AND c.claim_kind = 'request' AND c.status = 'confirmed'
             AND NOT EXISTS (
                 SELECT 1 FROM secretary_thread_claim_sources source_check
                 LEFT JOIN secretary_source_events event_check
                   ON event_check.source_event_id = source_check.source_event_id
                 LEFT JOIN secretary_conversations conversation_check
                   ON conversation_check.id = event_check.conversation_id
                 LEFT JOIN secretary_message_contents content_check
                   ON content_check.source_event_id = event_check.source_event_id
                 LEFT JOIN secretary_message_tombstones tombstone_check
                   ON tombstone_check.source_event_id = event_check.source_event_id
                  AND tombstone_check.account_id = event_check.account_id
                  AND tombstone_check.status = 'applied'
                 WHERE source_check.claim_id = c.claim_id
                   AND (event_check.source_event_id IS NULL
                        OR event_check.account_id <> t.account_id
                        OR conversation_check.memory_mode <> 'normal'
                        OR content_check.source_event_id IS NULL
                        OR content_check.content_mode <> 'normal'
                        OR tombstone_check.source_event_id IS NOT NULL)
             )
           GROUP BY c.claim_id, c.claimant_actor_id, c.claim_kind, c.status
           LIMIT 5"#,
        [thread.thread_id.as_str().into(), account_id.into()],
    ))
    .all(db)
    .await
    .map_err(store_error)?;
    rows.into_iter()
        .map(|row| {
            Ok(ConfirmedRequesterData {
                claimant_actor_id: row.claimant_actor_id,
                actor_kind: row.actor_kind,
                source_event_ids: parse_source_event_id_list(row.source_event_ids)?,
            })
        })
        .collect()
}

#[derive(Debug, FromQueryResult)]
struct ConfirmedCommitmentRow {
    fact_json: String,
    source_event_ids: Option<String>,
}

/// 已确认承诺的类型化数据（promisor / beneficiary + 来源）。
struct ConfirmedCommitmentData {
    promisor: String,
    promisor_kind: String,
    beneficiary: String,
    beneficiary_kind: String,
    source_event_ids: Vec<SourceEventId>,
}

/// 已确认承诺记忆（与线程来源事件关联的 confirmed commitment；只保留 Pending 状态）。
async fn load_confirmed_commitments(
    db: &DatabaseConnection,
    account_id: u64,
    thread: Option<&CausalThreadData>,
) -> Result<Vec<ConfirmedCommitmentData>, InboundEventStoreError> {
    let Some(thread) = thread else {
        return Ok(Vec::new());
    };
    let rows = ConfirmedCommitmentRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT CAST(f.fact_json AS CHAR) AS fact_json,
                  GROUP_CONCAT(s.source_event_id SEPARATOR ',') AS source_event_ids
           FROM secretary_memory_facts f
           JOIN secretary_memory_fact_sources s ON s.fact_id = f.fact_id
           -- 线程成员关系取有效线程视图（合并/拆分后），不返回旧 Thread 的承诺关系。
           JOIN secretary_effective_thread_events te ON te.source_event_id = s.source_event_id
           WHERE f.account_id = ? AND f.fact_kind = 'commitment' AND f.fact_status = 'confirmed'
             AND (f.valid_until_unix_secs IS NULL OR f.valid_until_unix_secs > ?)
             AND te.thread_id = ?
             -- envelope_only / never_long_term / 正文投影缺失 / 已召回来源不得支撑人物长期
             -- 事实（约束 6/7）。LEFT JOIN 未命中时 m2.source_event_id 为 NULL，必须显式
             -- 判为受限（fail-closed）。
             AND NOT EXISTS (
                 SELECT 1 FROM secretary_memory_fact_sources fs2
                 JOIN secretary_source_events e2 ON e2.source_event_id = fs2.source_event_id
                 JOIN secretary_conversations c2 ON c2.id = e2.conversation_id
                 LEFT JOIN secretary_message_contents m2
                   ON m2.source_event_id = fs2.source_event_id
                 WHERE fs2.fact_id = f.fact_id
                   AND (c2.memory_mode <> 'normal'
                        OR m2.content_mode <> 'normal'
                        OR m2.source_event_id IS NULL)
             )
             AND NOT EXISTS (
                 SELECT 1 FROM secretary_memory_fact_sources fs3
                 JOIN secretary_message_tombstones t3
                   ON t3.source_event_id = fs3.source_event_id
                 WHERE fs3.fact_id = f.fact_id AND t3.status = 'applied'
             )
           GROUP BY f.fact_id, f.fact_json
           LIMIT 4"#,
        [
            account_id.into(),
            Utc::now().timestamp().into(),
            thread.thread_id.as_str().into(),
        ],
    ))
    .all(db)
    .await
    .map_err(store_error)?;
    let mut commitments = Vec::with_capacity(rows.len());
    for row in rows {
        let payload: crate::CommitmentMemory =
            serde_json::from_str(&row.fact_json).map_err(|error| {
                InboundEventStoreError::InvalidData(format!(
                    "confirmed commitment fact_json decode failed: {error}"
                ))
            })?;
        // 只保留活跃承诺；已完成/已取消的承诺不支撑 PromisedBy/Benefits。
        if payload.status != crate::CommitmentStatus::Pending {
            continue;
        }
        let promisor_kind = load_actor_kind_from_events(db, account_id, &payload.promisor.actor_id)
            .await?
            .unwrap_or_else(|| "external".into());
        let beneficiary_kind =
            load_actor_kind_from_events(db, account_id, &payload.beneficiary.actor_id)
                .await?
                .unwrap_or_else(|| "external".into());
        commitments.push(ConfirmedCommitmentData {
            promisor: payload.promisor.actor_id,
            promisor_kind,
            beneficiary: payload.beneficiary.actor_id,
            beneficiary_kind,
            source_event_ids: parse_source_event_id_list(row.source_event_ids)?,
        });
    }
    Ok(commitments)
}

#[derive(Debug, FromQueryResult)]
struct ProfileRow {
    platform_identity_kind: String,
    display_name: String,
    aliases_json: String,
    trust: String,
    confirmed: i8,
    invalidated: i8,
    source_event_ids_json: String,
    established_by_event_id: Option<String>,
}

/// 账号内稳定 ID 的全部 current 档案行（跨身份命名空间）。
/// 身份 = account + 身份种类 + 稳定 ID，唯一键含身份种类：同账号下不同命名空间的
/// 相同稳定 ID 是不同参与者，调用方负责在冲突时 fail-closed。
async fn load_current_profiles(
    db: &DatabaseConnection,
    account_id: u64,
    actor_id: &str,
) -> Result<Vec<ProfileRow>, InboundEventStoreError> {
    ProfileRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT platform_identity_kind, display_name,
                  CAST(aliases_json AS CHAR) AS aliases_json, trust,
                  confirmed, invalidated,
                  CAST(source_event_ids_json AS CHAR) AS source_event_ids_json,
                  established_by_event_id
           FROM secretary_participant_profiles
           WHERE account_id = ? AND actor_platform_id = ? AND current = 1
           ORDER BY platform_identity_kind"#,
        [account_id.into(), actor_id.into()],
    ))
    .all(db)
    .await
    .map_err(store_error)
}

/// 按完整三元组身份（account + 身份种类 + 稳定 ID）精确读取 current 档案行。
/// 唯一键含身份种类，同账号下至多一行；用于 by-ref 查询，无歧义可能。
async fn load_current_profile_by_kind(
    db: &DatabaseConnection,
    account_id: u64,
    platform_identity_kind: &str,
    actor_id: &str,
) -> Result<Option<ProfileRow>, InboundEventStoreError> {
    ProfileRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT platform_identity_kind, display_name,
                  CAST(aliases_json AS CHAR) AS aliases_json, trust,
                  confirmed, invalidated,
                  CAST(source_event_ids_json AS CHAR) AS source_event_ids_json,
                  established_by_event_id
           FROM secretary_participant_profiles
           WHERE account_id = ? AND platform_identity_kind = ?
             AND actor_platform_id = ? AND current = 1
           LIMIT 1"#,
        [
            account_id.into(),
            platform_identity_kind.into(),
            actor_id.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(store_error)
}

#[derive(Debug, FromQueryResult)]
struct ConversationObservationRow {
    group_card: Option<String>,
    group_role: String,
    established_by_event_id: Option<String>,
    source_event_ids_json: String,
}

/// 会话作用域观察（群名片/群角色）：按 (account, conversation, identity_kind, actor)
/// 精确命中，绝不跨会话猜测；行已失效（invalidated=1）时视为无观察。
async fn load_conversation_observation(
    db: &DatabaseConnection,
    account_id: u64,
    conversation_id: u64,
    platform_identity_kind: &str,
    actor_id: &str,
) -> Result<Option<ConversationObservationRow>, InboundEventStoreError> {
    ConversationObservationRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT group_card, group_role, established_by_event_id,
                  CAST(source_event_ids_json AS CHAR) AS source_event_ids_json
           FROM secretary_participant_conversation_observations
           WHERE account_id = ? AND conversation_id = ?
             AND platform_identity_kind = ? AND actor_platform_id = ?
             AND invalidated = 0
           LIMIT 1"#,
        [
            account_id.into(),
            conversation_id.into(),
            platform_identity_kind.into(),
            actor_id.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(store_error)
}

/// ConversationRef → 账号作用域内的会话行 ID（不存在时返回 None，不报错：
/// 观察证据缺失按"无该会话观察"处理，跨账号绝不关联）。
async fn resolve_conversation_id(
    db: &DatabaseConnection,
    account_id: u64,
    conversation: &ConversationRef,
) -> Result<Option<u64>, InboundEventStoreError> {
    let row = AccountIdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT id
           FROM secretary_conversations
           WHERE account_id = ? AND conversation_kind = ? AND platform_conversation_id = ?"#,
        [
            account_id.into(),
            conversation.kind.as_str().into(),
            conversation.id.clone().into(),
        ],
    ))
    .one(db)
    .await
    .map_err(store_error)?;
    Ok(row.map(|row| row.id))
}

/// 线程 → 根事件所在会话（线程无会话列，取根事件会话作为群属性作用域）。
async fn resolve_thread_conversation_id(
    db: &DatabaseConnection,
    account_id: u64,
    thread_id: &EventThreadId,
) -> Result<Option<u64>, InboundEventStoreError> {
    let row = AccountIdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT e.conversation_id AS id
           FROM secretary_event_threads t
           JOIN secretary_source_events e ON e.source_event_id = t.root_event_id
           WHERE t.thread_id = ? AND t.account_id = ?
           LIMIT 1"#,
        [thread_id.as_str().into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(store_error)?;
    Ok(row.map(|row| row.id))
}

/// 单个建立事件的独立有效性（P0：当前值必须由建立事件支撑，不受有界来源列表
/// 淘汰影响）。语义与 source_refs_valid 一致：事件必须存在、无撤回、非
/// normal，且正文投影存在。
async fn single_event_valid(
    db: &DatabaseConnection,
    account_id: u64,
    event_id: &str,
) -> Result<bool, InboundEventStoreError> {
    let json = serde_json::json!([event_id]).to_string();
    source_refs_valid(db, account_id, &json).await
}

/// 参与者档案保存的是信封级身份观察，因此 envelope_only 来源仍可支撑昵称；
/// never_long_term、撤回、投影缺失或跨账号来源仍必须使档案 fail-closed。
async fn profile_event_valid(
    db: &DatabaseConnection,
    account_id: u64,
    event_id: &str,
) -> Result<bool, InboundEventStoreError> {
    let json = serde_json::json!([event_id]).to_string();
    profile_source_refs_valid(db, account_id, &json).await
}

async fn profile_source_refs_valid(
    db: &DatabaseConnection,
    account_id: u64,
    source_json: &str,
) -> Result<bool, InboundEventStoreError> {
    if source_json.trim().is_empty() || source_json == "null" {
        return Ok(false);
    }
    let sources: Vec<String> = serde_json::from_str(source_json).map_err(|error| {
        InboundEventStoreError::InvalidData(format!("source_event_ids_json decode failed: {error}"))
    })?;
    if sources.is_empty() {
        return Ok(false);
    }
    #[derive(Debug, FromQueryResult)]
    struct ValidityRow {
        valid: i8,
    }
    let row = ValidityRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT NOT EXISTS (
             SELECT 1 FROM JSON_TABLE(CAST(? AS CHAR), '$[*]'
                 COLUMNS (sid VARCHAR(191) PATH '$')) jt
             LEFT JOIN secretary_source_events e
               ON e.source_event_id = jt.sid AND e.account_id = ?
             LEFT JOIN secretary_conversations c ON c.id = e.conversation_id
             LEFT JOIN secretary_message_contents m ON m.source_event_id = jt.sid
             LEFT JOIN secretary_message_tombstones t
               ON t.source_event_id = jt.sid AND t.status = 'applied'
             WHERE e.source_event_id IS NULL
                OR t.source_event_id IS NOT NULL
                OR c.memory_mode NOT IN ('normal', 'envelope_only')
                OR m.content_mode NOT IN ('normal', 'envelope_only')
                OR m.source_event_id IS NULL
           ) AS valid"#,
        [source_json.into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(store_error)?;
    Ok(row.map(|row| row.valid != 0).unwrap_or(false))
}

/// 来源有效性（fail-closed）：来源事件消失、已被撤回、正文投影缺失或会话/事件为
/// 非 normal 时，该档案/观察不得作为跨会话事实返回。来源列表为空视为无效。
async fn source_refs_valid(
    db: &DatabaseConnection,
    account_id: u64,
    source_json: &str,
) -> Result<bool, InboundEventStoreError> {
    if source_json.trim().is_empty() || source_json == "null" {
        return Ok(false);
    }
    let sources: Vec<String> = serde_json::from_str(source_json).map_err(|error| {
        InboundEventStoreError::InvalidData(format!("source_event_ids_json decode failed: {error}"))
    })?;
    if sources.is_empty() {
        return Ok(false);
    }
    #[derive(Debug, FromQueryResult)]
    struct ValidityRow {
        valid: i8,
    }
    let row = ValidityRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT NOT EXISTS (
             SELECT 1 FROM JSON_TABLE(CAST(? AS CHAR), '$[*]'
                 COLUMNS (sid VARCHAR(191) PATH '$')) jt
             LEFT JOIN secretary_source_events e
               ON e.source_event_id = jt.sid AND e.account_id = ?
             LEFT JOIN secretary_conversations c ON c.id = e.conversation_id
             LEFT JOIN secretary_message_contents m ON m.source_event_id = jt.sid
             LEFT JOIN secretary_message_tombstones t
               ON t.source_event_id = jt.sid AND t.status = 'applied'
             WHERE e.source_event_id IS NULL
                OR t.source_event_id IS NOT NULL
                OR c.memory_mode <> 'normal'
                OR m.content_mode <> 'normal'
                OR m.source_event_id IS NULL
           ) AS valid"#,
        [source_json.into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(store_error)?;
    Ok(row.map(|row| row.valid != 0).unwrap_or(false))
}

#[derive(Debug, FromQueryResult)]
struct ActorKindRow {
    actor_kind: String,
}

async fn load_actor_kind_from_events(
    db: &DatabaseConnection,
    account_id: u64,
    actor_id: &str,
) -> Result<Option<String>, InboundEventStoreError> {
    let row = ActorKindRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT e.actor_kind
           FROM secretary_source_events e
           JOIN secretary_conversations c ON c.id = e.conversation_id
           JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
           WHERE e.account_id = ? AND e.actor_platform_id = ?
             AND c.memory_mode = 'normal' AND m.content_mode = 'normal'
             AND NOT EXISTS (
                 SELECT 1 FROM secretary_message_tombstones tombstone
                 WHERE tombstone.source_event_id = e.source_event_id
                   AND tombstone.account_id = e.account_id
                   AND tombstone.status = 'applied'
             )
           ORDER BY e.occurred_at_unix_secs DESC, e.source_event_id DESC
           LIMIT 1"#,
        [account_id.into(), actor_id.into()],
    ))
    .one(db)
    .await
    .map_err(store_error)?;
    Ok(row.map(|row| row.actor_kind))
}

#[derive(Debug, FromQueryResult)]
struct RecentEventIdRow {
    source_event_id: String,
}

async fn load_related_event_ids(
    db: &DatabaseConnection,
    account_id: u64,
    actor_id: &str,
) -> Result<Vec<SourceEventId>, InboundEventStoreError> {
    let rows = RecentEventIdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT e.source_event_id
           FROM secretary_source_events e
           JOIN secretary_conversations c ON c.id = e.conversation_id
           JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
           WHERE e.account_id = ? AND e.actor_platform_id = ?
             AND c.memory_mode = 'normal' AND m.content_mode = 'normal'
             AND NOT EXISTS (
                 SELECT 1 FROM secretary_message_tombstones tombstone
                 WHERE tombstone.source_event_id = e.source_event_id
                   AND tombstone.account_id = e.account_id
                   AND tombstone.status = 'applied'
             )
           ORDER BY e.occurred_at_unix_secs DESC, e.source_event_id DESC
           LIMIT 10"#,
        [account_id.into(), actor_id.into()],
    ))
    .all(db)
    .await
    .map_err(store_error)?;
    rows.into_iter()
        .map(|row| SourceEventId::new(row.source_event_id).map_err(domain_err))
        .collect()
}

#[derive(Debug, FromQueryResult)]
struct PersonMemoryRow {
    fact_json: String,
    source_event_ids: Option<String>,
}

/// 已确认人物记忆 → 类型化属性（关系/职责/沟通偏好）。未批准候选绝不进入确认字段；
/// 已确认记忆必须携带来源；来源失效/过期后由上层调用方负责整体失效。
async fn load_confirmed_person_memory(
    db: &DatabaseConnection,
    account_id: u64,
    actor_id: &str,
) -> Result<Vec<PersonMemoryFactAttributes>, InboundEventStoreError> {
    let rows = PersonMemoryRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"SELECT CAST(f.fact_json AS CHAR) AS fact_json,
                  GROUP_CONCAT(s.source_event_id SEPARATOR ',') AS source_event_ids
           FROM secretary_memory_facts f
           JOIN secretary_memory_fact_sources s ON s.fact_id = f.fact_id
           WHERE f.account_id = ? AND f.fact_kind = 'person' AND f.fact_status = 'confirmed'
             AND (f.valid_until_unix_secs IS NULL OR f.valid_until_unix_secs > ?)
             AND JSON_UNQUOTE(JSON_EXTRACT(CAST(f.fact_json AS CHAR), '$.person.actor_id')) = ?
             -- envelope_only / never_long_term / 正文投影缺失 / 已召回来源不得支撑人物长期
             -- 事实（约束 6/7）。LEFT JOIN 未命中时 m2.source_event_id 为 NULL，必须显式
             -- 判为受限（fail-closed），否则已删除投影的来源仍会放行事实。
             AND NOT EXISTS (
                 SELECT 1 FROM secretary_memory_fact_sources fs2
                 JOIN secretary_source_events e2 ON e2.source_event_id = fs2.source_event_id
                 JOIN secretary_conversations c2 ON c2.id = e2.conversation_id
                 LEFT JOIN secretary_message_contents m2
                   ON m2.source_event_id = fs2.source_event_id
                 WHERE fs2.fact_id = f.fact_id
                   AND (c2.memory_mode <> 'normal'
                        OR m2.content_mode <> 'normal'
                        OR m2.source_event_id IS NULL)
             )
             AND NOT EXISTS (
                 SELECT 1 FROM secretary_memory_fact_sources fs3
                 JOIN secretary_message_tombstones t3
                   ON t3.source_event_id = fs3.source_event_id
                 WHERE fs3.fact_id = f.fact_id AND t3.status = 'applied'
             )
           GROUP BY f.fact_id, f.fact_json
           LIMIT 5"#,
        [
            account_id.into(),
            Utc::now().timestamp().into(),
            actor_id.into(),
        ],
    ))
    .all(db)
    .await
    .map_err(store_error)?;
    rows.into_iter()
        .map(|row| {
            let payload: crate::PersonMemory =
                serde_json::from_str(&row.fact_json).map_err(|error| {
                    InboundEventStoreError::InvalidData(format!(
                        "confirmed person memory fact_json decode failed: {error}"
                    ))
                })?;
            let sources = parse_source_event_id_list(row.source_event_ids)?;
            Ok(PersonMemoryFactAttributes { payload, sources })
        })
        .collect()
}

/// 已确认人物记忆的解析结果（携带来源事件）。
struct PersonMemoryFactAttributes {
    payload: crate::PersonMemory,
    sources: Vec<SourceEventId>,
}

impl PersonMemoryFactAttributes {
    fn into_attributes(self) -> Vec<ParticipantAttribute> {
        let mut attributes = Vec::new();
        if let Some(relationship) = self
            .payload
            .relationship
            .filter(|value| !value.trim().is_empty())
        {
            attributes.push(ParticipantAttribute {
                kind: ParticipantAttributeKind::Relationship,
                value: relationship.chars().take(200).collect(),
                trust: IdentityTrust::Verified,
                confirmed: true,
                source_event_ids: self.sources.clone(),
                directory_snapshot_id: None,
                invalidated: false,
                invalidation_reason: None,
            });
        }
        for responsibility in self.payload.responsibilities {
            if responsibility.trim().is_empty() {
                continue;
            }
            attributes.push(ParticipantAttribute {
                kind: ParticipantAttributeKind::Responsibility,
                value: responsibility.chars().take(200).collect(),
                trust: IdentityTrust::Verified,
                confirmed: true,
                source_event_ids: self.sources.clone(),
                directory_snapshot_id: None,
                invalidated: false,
                invalidation_reason: None,
            });
        }
        for preference in self.payload.communication_preferences {
            if preference.trim().is_empty() {
                continue;
            }
            attributes.push(ParticipantAttribute {
                kind: ParticipantAttributeKind::CommunicationPreference,
                value: preference.chars().take(200).collect(),
                trust: IdentityTrust::Verified,
                confirmed: true,
                source_event_ids: self.sources.clone(),
                directory_snapshot_id: None,
                invalidated: false,
                invalidation_reason: None,
            });
        }
        attributes
    }
}

// ===== 项目/承诺查询行模型（MEM-003/MEM-004）=====

#[derive(Debug, FromQueryResult)]
struct ProjectListRow {
    fact_id: String,
    fact_json: String,
    updated_at_unix: i64,
}

#[derive(Debug, FromQueryResult)]
struct ProjectDetailRow {
    fact_id: String,
    fact_json: String,
    confidence_bps: u16,
    valid_until_unix_secs: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct ProjectSourceCheckRow {
    #[allow(dead_code)]
    present: i64,
}

#[derive(Debug, FromQueryResult)]
struct CommitmentListRow {
    fact_id: String,
    fact_json: String,
    follow_up_id: Option<String>,
    follow_up_status: Option<String>,
}
