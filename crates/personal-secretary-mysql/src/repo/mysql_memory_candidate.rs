//! MySQL 记忆候选仓储：实现 [`MemoryCandidateStoreT`]。
//!
//! 领取批次时按内容信任策略过滤（normal 恒可提取；local_only 仅在调用方确认
//! loopback 端点时；envelope_only/never_long_term 与已 Applied 撤回事件排除），
//! 以持久化游标 + 租约做 fencing；提交时同账号同 fingerprint 只保留第一条。
//! 所有候选版本列均为 BIGINT UNSIGNED，行模型必须用 `u64`。

use async_trait::async_trait;
use chrono::{Duration, NaiveDateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, FromQueryResult,
    Statement, TransactionTrait,
};

use crate::{
    ContentTrustLevel, ConversationKind, ConversationRef, InboundEventStoreError, MemoryCandidate,
    MemoryCandidateBatch, MemoryCandidateCursor, MemoryCandidateEvent, MemoryCandidateId,
    MemoryCandidateKind, MemoryCandidateLeaseToken, MemoryCandidateSourceExcerpt,
    MemoryCandidateStatus, MemoryCandidateVersion, MemoryCandidateView, MessageRole,
    SourceAccountRef, SourceEventId, ThreadActorRef, validate_memory_candidate,
};

use super::mysql_inbound::store_error;

pub(crate) struct MySqlMemoryCandidateStore {
    db: DatabaseConnection,
}

impl MySqlMemoryCandidateStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// 本地模式领取延期（远程模式被过滤）的 local_only 事件批次。
    /// 主游标为 NULL（从未提交过任何批次）时不走延期路径：主领取会从全局起点
    /// 覆盖全部事件，延期行由主批次提交时清理。延期批次 next_cursor 恒等于当前
    /// 主游标——延期事件必在当前游标之前或等位（延期行只在本批次结束位置之内
    /// 产生，而主游标至少推进到该位置），提交只释放租约并删除已处理行，绝不
    /// 推进主游标，避免越过尚未提交的旧批次事件。
    #[allow(clippy::too_many_arguments)]
    async fn claim_deferred_batch(
        &self,
        transaction: &DatabaseTransaction,
        account: &SourceAccountRef,
        state: &CandidateStateRow,
        lease_token: MemoryCandidateLeaseToken,
        max_events: u32,
        max_event_chars: u32,
        max_total_chars: u32,
    ) -> Result<Option<MemoryCandidateBatch>, InboundEventStoreError> {
        // 本方法只在 allow_local_only=true（已验证 loopback 端点）时被调用。
        let trust_in = trust_clause(true);
        let Some(cursor_time) = state.last_received_at else {
            return Ok(None);
        };
        let Some(cursor_event_id) = state.last_source_event_id.as_deref() else {
            return Ok(None);
        };
        // 1) 清理已永久不可处理的延期行（撤回，或降级为 envelope/never_long_term）：
        //    与主领取同一套信任过滤，避免死行阻塞延期消费。
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                format!(
                    r#"
DELETE deferred FROM secretary_memory_candidate_deferred deferred
JOIN secretary_source_events event ON event.source_event_id = deferred.source_event_id
JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
WHERE deferred.account_id = ?
  AND (conversation.memory_mode NOT IN ({trust_in})
       OR content.content_mode NOT IN ({trust_in})
       OR EXISTS (
           SELECT 1 FROM secretary_message_tombstones tombstone
           WHERE tombstone.source_event_id = event.source_event_id
             AND tombstone.account_id = event.account_id
             AND tombstone.status = 'applied'
       ))
"#
                ),
                [state.account_id.into()],
            ))
            .await
            .map_err(store_error)?;

        // 2) 最早可处理延期事件所属的会话（按 received_at 全局顺序）。
        let conversation_row =
            FirstConversationRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                format!(
                    r#"
SELECT event.conversation_id, conversation.conversation_kind,
       conversation.platform_conversation_id
FROM secretary_memory_candidate_deferred deferred
JOIN secretary_source_events event ON event.source_event_id = deferred.source_event_id
JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
WHERE deferred.account_id = ?
  AND conversation.memory_mode IN ({trust_in})
  AND content.content_mode IN ({trust_in})
  AND NOT EXISTS (
      SELECT 1 FROM secretary_message_tombstones tombstone
      WHERE tombstone.source_event_id = event.source_event_id
        AND tombstone.account_id = event.account_id
        AND tombstone.status = 'applied'
  )
ORDER BY deferred.received_at ASC, deferred.source_event_id ASC
LIMIT 1
"#
                ),
                [state.account_id.into()],
            ))
            .one(transaction)
            .await
            .map_err(store_error)?;
        let Some(conversation_row) = conversation_row else {
            // 延期队列为空（或全部不可处理）；交给主领取。
            return Ok(None);
        };
        let conversation = ConversationRef::new(
            parse_conversation_kind(&conversation_row.conversation_kind)?,
            conversation_row.platform_conversation_id,
        )
        .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;

        // 3) 切换点：延期队列中第一个不属于本批会话的事件（连续同会话前缀）。
        let switch_row = SwitchPointRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            format!(
                r#"
SELECT event.source_event_id, event.received_at
FROM secretary_memory_candidate_deferred deferred
JOIN secretary_source_events event ON event.source_event_id = deferred.source_event_id
JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
WHERE deferred.account_id = ?
  AND event.conversation_id <> ?
  AND conversation.memory_mode IN ({trust_in})
  AND content.content_mode IN ({trust_in})
  AND NOT EXISTS (
      SELECT 1 FROM secretary_message_tombstones tombstone
      WHERE tombstone.source_event_id = event.source_event_id
        AND tombstone.account_id = event.account_id
        AND tombstone.status = 'applied'
  )
ORDER BY deferred.received_at ASC, deferred.source_event_id ASC
LIMIT 1
"#
            ),
            [
                state.account_id.into(),
                conversation_row.conversation_id.into(),
            ],
        ))
        .one(transaction)
        .await
        .map_err(store_error)?;
        let switch_bound: [sea_orm::Value; 4] = match &switch_row {
            Some(switch) => [
                switch.received_at.into(),
                switch.received_at.into(),
                switch.received_at.into(),
                switch.source_event_id.clone().into(),
            ],
            None => [
                sea_orm::Value::ChronoDateTime(None),
                sea_orm::Value::ChronoDateTime(None),
                sea_orm::Value::ChronoDateTime(None),
                sea_orm::Value::String(None),
            ],
        };

        // 4) 该会话的延期事件（按全局顺序，受切换点约束）。
        let rows = CandidateEventRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            format!(
                r#"
SELECT event.source_event_id, event.actor_platform_id, event.message_role,
       event.occurred_at_unix_secs, event.received_at,
       CASE
         WHEN conversation.memory_mode = 'never_long_term' OR content.content_mode = 'never_long_term'
           THEN 'never_long_term'
         WHEN conversation.memory_mode = 'envelope_only' OR content.content_mode = 'envelope_only'
           THEN 'envelope_only'
         WHEN conversation.memory_mode = 'local_only' OR content.content_mode = 'local_only'
           THEN 'local_only'
         ELSE COALESCE(conversation.memory_mode, 'normal')
       END AS content_trust_level,
       SUBSTRING(content.normalized_text, 1, ?) AS normalized_text
FROM secretary_memory_candidate_deferred deferred
JOIN secretary_source_events event ON event.source_event_id = deferred.source_event_id
JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
WHERE deferred.account_id = ?
  AND event.conversation_id = ?
  AND conversation.memory_mode IN ({trust_in})
  AND content.content_mode IN ({trust_in})
  AND NOT EXISTS (
      SELECT 1 FROM secretary_message_tombstones tombstone
      WHERE tombstone.source_event_id = event.source_event_id
        AND tombstone.account_id = event.account_id
        AND tombstone.status = 'applied'
  )
  AND (? IS NULL
       OR event.received_at < ?
       OR (event.received_at = ? AND event.source_event_id < ?))
ORDER BY deferred.received_at ASC, deferred.source_event_id ASC
LIMIT ?
"#
            ),
            [
                (max_event_chars + 1).into(),
                state.account_id.into(),
                conversation_row.conversation_id.into(),
                switch_bound[0].clone(),
                switch_bound[1].clone(),
                switch_bound[2].clone(),
                switch_bound[3].clone(),
                max_events.into(),
            ],
        ))
        .all(transaction)
        .await
        .map_err(store_error)?;
        if rows.is_empty() {
            // 会话由最早延期事件选定，理论上必有事件；防御性返回，交给主领取。
            return Ok(None);
        }
        let events = build_candidate_events(account, rows, max_event_chars, max_total_chars)?;
        Ok(Some(MemoryCandidateBatch {
            account: account.clone(),
            conversation,
            lease_token,
            events,
            next_cursor: MemoryCandidateCursor {
                received_at_unix_micros: cursor_time.and_utc().timestamp_micros(),
                source_event_id: SourceEventId::new(cursor_event_id.to_owned())?,
            },
        }))
    }
}

#[async_trait]
impl crate::MemoryCandidateStoreT for MySqlMemoryCandidateStore {
    async fn claim_candidate_batch(
        &self,
        account: &SourceAccountRef,
        max_events: u32,
        max_event_chars: u32,
        max_total_chars: u32,
        lease_secs: u64,
        allow_local_only: bool,
    ) -> Result<Option<MemoryCandidateBatch>, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let trust_in = trust_clause(allow_local_only);
        let state = CandidateStateRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"
SELECT account.id AS account_id, state.last_received_at, state.last_source_event_id
FROM secretary_accounts account
LEFT JOIN secretary_memory_candidate_processing_state state
    ON state.account_id = account.id
WHERE account.source_channel = ? AND account.platform_account_id = ?
  AND account.status = 'active'
  AND (state.lease_token IS NULL OR state.lease_expires_at < ?)
FOR UPDATE SKIP LOCKED
"#,
            [
                account.channel.as_str().into(),
                account.account_id.clone().into(),
                now.into(),
            ],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?;
        let Some(state) = state else {
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        };

        let lease_token = MemoryCandidateLeaseToken::generate();
        let lease_expires_at = now + Duration::seconds(lease_secs as i64);
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"
INSERT INTO secretary_memory_candidate_processing_state
    (account_id, last_received_at, last_source_event_id, lease_token, lease_expires_at,
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
                    state.account_id.into(),
                    lease_token.as_str().into(),
                    lease_expires_at.into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;

        // 本地模式优先消费被远程模式过滤后延期的 local_only 事件：账号全局
        // 游标可能已推进过它们，主领取永远看不到；延期消费不推进游标，只删除
        // 已处理行（提交时清理）。
        if allow_local_only
            && let Some(batch) = self
                .claim_deferred_batch(
                    &transaction,
                    account,
                    &state,
                    lease_token.clone(),
                    max_events,
                    max_event_chars,
                    max_total_chars,
                )
                .await?
        {
            transaction.commit().await.map_err(store_error)?;
            return Ok(Some(batch));
        }

        // 批次按会话分界：先定位游标之后第一个可提取事件所属的会话，再只加载
        // 全局排序中与该会话**连续**的事件（直到遇到下一个其他会话的事件为止）。
        // 只处理连续同会话前缀，游标推进就不会越过交错在其他会话之间的事件，
        // 避免 A1、B1、A2 这类序列中 B1 被永久跳过。
        let conversation_row =
            FirstConversationRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                format!(
                    r#"
SELECT event.conversation_id, conversation.conversation_kind,
       conversation.platform_conversation_id
FROM secretary_source_events event
JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
WHERE event.account_id = ?
  AND conversation.memory_mode IN ({trust_in})
  AND content.content_mode IN ({trust_in})
  AND NOT EXISTS (
      SELECT 1 FROM secretary_message_tombstones tombstone
      WHERE tombstone.source_event_id = event.source_event_id
        AND tombstone.account_id = event.account_id
        AND tombstone.status = 'applied'
  )
  AND (? IS NULL
       OR event.received_at > ?
       OR (event.received_at = ? AND event.source_event_id > ?))
ORDER BY event.received_at ASC, event.source_event_id ASC
LIMIT 1
"#
                ),
                [
                    state.account_id.into(),
                    state.last_received_at.into(),
                    state.last_received_at.into(),
                    state.last_received_at.into(),
                    state.last_source_event_id.clone().into(),
                ],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?;
        let Some(conversation_row) = conversation_row else {
            // 游标之后没有可提取事件；释放租约并结束本轮。
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "UPDATE secretary_memory_candidate_processing_state \
                     SET lease_token = NULL, lease_expires_at = NULL, last_error = NULL, \
                         updated_at = ? WHERE account_id = ? AND lease_token = ?",
                    [
                        now.into(),
                        state.account_id.into(),
                        lease_token.as_str().into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        };
        let conversation = ConversationRef::new(
            parse_conversation_kind(&conversation_row.conversation_kind)?,
            conversation_row.platform_conversation_id,
        )
        .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;

        // 会话切换点：全局排序中第一个不属于本会话的可提取事件。本批事件严格
        // 小于该切换点（双元组比较），保证只取连续同会话前缀；无切换点（本会话
        // 一直延续到游标尽头）时以 NULL 放行全部。
        let switch_row = SwitchPointRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            format!(
                r#"
SELECT event.source_event_id, event.received_at
FROM secretary_source_events event
JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
WHERE event.account_id = ?
  AND event.conversation_id <> ?
  AND conversation.memory_mode IN ({trust_in})
  AND content.content_mode IN ({trust_in})
  AND NOT EXISTS (
      SELECT 1 FROM secretary_message_tombstones tombstone
      WHERE tombstone.source_event_id = event.source_event_id
        AND tombstone.account_id = event.account_id
        AND tombstone.status = 'applied'
  )
  AND (? IS NULL
       OR event.received_at > ?
       OR (event.received_at = ? AND event.source_event_id > ?))
ORDER BY event.received_at ASC, event.source_event_id ASC
LIMIT 1
"#
            ),
            [
                state.account_id.into(),
                conversation_row.conversation_id.into(),
                state.last_received_at.into(),
                state.last_received_at.into(),
                state.last_received_at.into(),
                state.last_source_event_id.clone().into(),
            ],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?;

        // 事件正文按 max_event_chars + 1 截取后加载，超过即整条省略，
        // 避免单条超大正文进入内存；整批字符预算在映射阶段再扣减。
        // 本会话事件严格小于切换点（NULL 表示无切换点，放行全部）。
        let switch_bound: [sea_orm::Value; 4] = match &switch_row {
            Some(switch) => [
                switch.received_at.into(),
                switch.received_at.into(),
                switch.received_at.into(),
                switch.source_event_id.clone().into(),
            ],
            None => [
                sea_orm::Value::ChronoDateTime(None),
                sea_orm::Value::ChronoDateTime(None),
                sea_orm::Value::ChronoDateTime(None),
                sea_orm::Value::String(None),
            ],
        };
        let rows = CandidateEventRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            format!(
                r#"
SELECT event.source_event_id, event.actor_platform_id, event.message_role,
       event.occurred_at_unix_secs, event.received_at,
       CASE
         WHEN conversation.memory_mode = 'never_long_term' OR content.content_mode = 'never_long_term'
           THEN 'never_long_term'
         WHEN conversation.memory_mode = 'envelope_only' OR content.content_mode = 'envelope_only'
           THEN 'envelope_only'
         WHEN conversation.memory_mode = 'local_only' OR content.content_mode = 'local_only'
           THEN 'local_only'
         ELSE COALESCE(conversation.memory_mode, 'normal')
       END AS content_trust_level,
       SUBSTRING(content.normalized_text, 1, ?) AS normalized_text
FROM secretary_source_events event
JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
WHERE event.account_id = ?
  AND event.conversation_id = ?
  AND conversation.memory_mode IN ({trust_in})
  AND content.content_mode IN ({trust_in})
  AND NOT EXISTS (
      SELECT 1 FROM secretary_message_tombstones tombstone
      WHERE tombstone.source_event_id = event.source_event_id
        AND tombstone.account_id = event.account_id
        AND tombstone.status = 'applied'
  )
  AND (? IS NULL
       OR event.received_at > ?
       OR (event.received_at = ? AND event.source_event_id > ?))
  AND (? IS NULL
       OR event.received_at < ?
       OR (event.received_at = ? AND event.source_event_id < ?))
ORDER BY event.received_at ASC, event.source_event_id ASC
LIMIT ?
"#
            ),
            [
                (max_event_chars + 1).into(),
                state.account_id.into(),
                conversation_row.conversation_id.into(),
                state.last_received_at.into(),
                state.last_received_at.into(),
                state.last_received_at.into(),
                state.last_source_event_id.clone().into(),
                switch_bound[0].clone(),
                switch_bound[1].clone(),
                switch_bound[2].clone(),
                switch_bound[3].clone(),
                max_events.into(),
            ],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?;
        let Some(last) = rows.last() else {
            // 会话内没有可提取事件（理论上不会发生：会话由首个事件选定）；
            // 按不可见数据错误处理，避免游标卡死。
            return Err(InboundEventStoreError::InvalidData(
                "candidate batch conversation has no claimable events".into(),
            ));
        };
        // 提前取下游标字段，避免 last 借用与后续按值迭代 rows 冲突。
        let next_cursor = MemoryCandidateCursor {
            received_at_unix_micros: last.received_at.and_utc().timestamp_micros(),
            source_event_id: SourceEventId::new(last.source_event_id.clone())?,
        };

        // 远程模式把本批范围内被过滤的 local_only 事件持久化为延期行（幂等），
        // 否则游标推进后这些事件对任何模式都永久不可达。范围限定在当前批次
        // 结束位置之内（全局二元组比较），保证提交后所有延期行都在主游标之前
        // 或等位，主领取永远不会重复读到它们。
        if !allow_local_only {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
INSERT IGNORE INTO secretary_memory_candidate_deferred
    (account_id, source_event_id, received_at, created_at)
SELECT event.account_id, event.source_event_id, event.received_at, ?
FROM secretary_source_events event
JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
WHERE event.account_id = ?
  AND (? IS NULL
       OR event.received_at > ?
       OR (event.received_at = ? AND event.source_event_id > ?))
  AND (event.received_at < ?
       OR (event.received_at = ? AND event.source_event_id < ?))
  AND CASE
        WHEN conversation.memory_mode = 'never_long_term' OR content.content_mode = 'never_long_term'
          THEN 'never_long_term'
        WHEN conversation.memory_mode = 'envelope_only' OR content.content_mode = 'envelope_only'
          THEN 'envelope_only'
        WHEN conversation.memory_mode = 'local_only' OR content.content_mode = 'local_only'
          THEN 'local_only'
        ELSE COALESCE(conversation.memory_mode, 'normal')
      END = 'local_only'
  AND NOT EXISTS (
      SELECT 1 FROM secretary_message_tombstones tombstone
      WHERE tombstone.source_event_id = event.source_event_id
        AND tombstone.account_id = event.account_id
        AND tombstone.status = 'applied'
  )
"#,
                    [
                        now.into(),
                        state.account_id.into(),
                        state.last_received_at.into(),
                        state.last_received_at.into(),
                        state.last_received_at.into(),
                        state.last_source_event_id.clone().into(),
                        last.received_at.into(),
                        last.received_at.into(),
                        last.source_event_id.clone().into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
        }

        let events = build_candidate_events(account, rows, max_event_chars, max_total_chars)?;

        transaction.commit().await.map_err(store_error)?;
        tracing::debug!(
            account = account.account_id,
            lease_token = %lease_token.as_str(),
            events = events.len(),
            omitted_events = events.iter().filter(|event| event.content_omitted).count(),
            "已领取有界记忆候选提取批次"
        );
        Ok(Some(MemoryCandidateBatch {
            account: account.clone(),
            conversation,
            lease_token,
            events,
            next_cursor,
        }))
    }

    async fn commit_candidates(
        &self,
        batch: &MemoryCandidateBatch,
        candidates: &[MemoryCandidate],
    ) -> Result<u64, InboundEventStoreError> {
        for candidate in candidates {
            validate_memory_candidate(candidate, batch)
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        }
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        // account_id 是 secretary_accounts 的数字主键，不能用平台账号 ID 字符串比较。
        let account_id =
            super::mysql_retriever::resolve_account_id(&self.db, &batch.account).await?;
        let owned = StateCountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT COUNT(*) AS value FROM secretary_memory_candidate_processing_state \
             WHERE account_id = ? AND lease_token = ? AND lease_expires_at >= ? FOR UPDATE",
            [
                account_id.into(),
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

        let mut inserted = 0u64;
        for candidate in candidates {
            // 先按指纹判定重复：只有"同账号同 fingerprint 已存在"允许静默跳过
            // （重复扫描 / 重启 / 模型重复输出）；其余 0 影响行（主键碰撞、
            // 其他唯一键冲突）一律视为数据错误，禁止静默吞掉后推进游标。
            let duplicate = FingerprintRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT 1 AS value FROM secretary_memory_candidates \
                 WHERE account_id = ? AND deterministic_fingerprint = ? LIMIT 1",
                [
                    account_id.into(),
                    candidate.deterministic_fingerprint.clone().into(),
                ],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?
            .is_some();
            if duplicate {
                continue;
            }
            let payload_json = serde_json::to_string(&candidate.payload).map_err(|error| {
                InboundEventStoreError::InvalidData(format!("cannot serialize candidate: {error}"))
            })?;
            let result = transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"
INSERT IGNORE INTO secretary_memory_candidates
    (candidate_id, account_id, candidate_kind, subject_key, payload_json, candidate_status,
     candidate_version, extractor_version, deterministic_fingerprint)
VALUES (?, ?, ?, ?, ?, 'proposed', 1, ?, ?)
"#,
                    [
                        candidate.candidate_id.as_str().into(),
                        account_id.into(),
                        candidate.payload.kind().into(),
                        candidate.subject_key.clone().into(),
                        payload_json.into(),
                        candidate.extractor_version.clone().into(),
                        candidate.deterministic_fingerprint.clone().into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
            if result.rows_affected() != 1 {
                return Err(InboundEventStoreError::InvalidData(
                    "memory candidate row was ignored without a known fingerprint duplicate".into(),
                ));
            }
            inserted += 1;
            for source in &candidate.sources {
                transaction
                    .execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::MySql,
                        r#"
INSERT INTO secretary_memory_candidate_sources
    (candidate_id, source_event_id, account_id, actor_platform_id, content_trust_level,
     occurred_at_unix_secs)
VALUES (?, ?, ?, ?, ?, ?)
"#,
                        [
                            candidate.candidate_id.as_str().into(),
                            source.source_event_id.as_str().into(),
                            account_id.into(),
                            source.actor.actor_id.clone().into(),
                            source.content_trust_level.as_str().into(),
                            source.occurred_at_unix_secs.into(),
                        ],
                    ))
                    .await
                    .map_err(store_error)?;
            }
        }

        // 清理本批次已消费事件的延期行：延期批次删除被消费事件；本地主批次
        // 覆盖游标 NULL 边界的同事件延期行；远程主批次只含 normal 事件，恒为空操作。
        if !batch.events.is_empty() {
            let placeholders = batch
                .events
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let mut params: Vec<sea_orm::Value> = vec![account_id.into()];
            params.extend(
                batch
                    .events
                    .iter()
                    .map(|event| event.source_event_id.as_str().into()),
            );
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    format!(
                        "DELETE FROM secretary_memory_candidate_deferred \
                         WHERE account_id = ? AND source_event_id IN ({placeholders})"
                    ),
                    params,
                ))
                .await
                .map_err(store_error)?;
        }

        let cursor_time =
            chrono::DateTime::from_timestamp_micros(batch.next_cursor.received_at_unix_micros)
                .ok_or_else(|| {
                    InboundEventStoreError::InvalidData("invalid candidate cursor time".into())
                })?
                .naive_utc();
        let updated = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"
UPDATE secretary_memory_candidate_processing_state
SET last_received_at = ?, last_source_event_id = ?, lease_token = NULL,
    lease_expires_at = NULL, last_error = NULL, updated_at = ?
WHERE account_id = ? AND lease_token = ?
"#,
                [
                    cursor_time.into(),
                    batch.next_cursor.source_event_id.as_str().into(),
                    now.into(),
                    account_id.into(),
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
            account = batch.account.account_id,
            candidates = candidates.len(),
            inserted,
            "记忆候选批次已原子提交，游标已推进"
        );
        Ok(inserted)
    }

    async fn release_candidate_claim(
        &self,
        lease_token: &MemoryCandidateLeaseToken,
        error: &str,
    ) -> Result<(), InboundEventStoreError> {
        let safe_error: String = error.chars().take(512).collect();
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_memory_candidate_processing_state SET lease_token = NULL, \
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

    async fn invalidate_stale_proposed(
        &self,
        account: &SourceAccountRef,
        limit: u32,
    ) -> Result<u64, InboundEventStoreError> {
        if !(1..=10_000).contains(&limit) {
            return Err(InboundEventStoreError::InvalidData(
                "candidate invalidation limit must be in 1..=10000".into(),
            ));
        }
        let account_id = super::mysql_retriever::resolve_account_id(&self.db, account).await?;
        // 失效条件与批准复验一致：任一条来源失效（撤回 tombstone 已 Applied、
        // 会话/正文切换为 envelope_only/never_long_term、来源跨账号、来源事件/
        // 会话/正文投影行缺失）就 invalidated，不能只在"所有来源均失效"时才失效
        // ——部分来源失效的候选每次审批都会失败，会永久卡在 proposed 且无法被
        // Owner 关闭。正文投影行可能因隐私或保留策略被单独删除，必须用
        // LEFT JOIN 并把缺失本身视为失效，否则 JOIN 无结果反而躲过失效。
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"
UPDATE secretary_memory_candidates candidate
SET candidate_status = 'invalidated', candidate_version = candidate_version + 1,
    updated_at = CURRENT_TIMESTAMP(6)
WHERE candidate.account_id = ?
  AND candidate.candidate_status = 'proposed'
  AND (
      NOT EXISTS (
          SELECT 1 FROM secretary_memory_candidate_sources source
          WHERE source.candidate_id = candidate.candidate_id
      )
      OR EXISTS (
          SELECT 1 FROM secretary_memory_candidate_sources source
          LEFT JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
          LEFT JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
          LEFT JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
          WHERE source.candidate_id = candidate.candidate_id
            AND (
                event.source_event_id IS NULL
                OR event.account_id <> candidate.account_id
                OR conversation.id IS NULL
                OR conversation.memory_mode <> 'normal'
                OR content.source_event_id IS NULL
                OR content.content_mode <> 'normal'
                OR EXISTS (
                    SELECT 1 FROM secretary_message_tombstones tombstone
                    WHERE tombstone.source_event_id = event.source_event_id
                      AND tombstone.account_id = event.account_id
                      AND tombstone.status = 'applied'
                )
            )
      )
  )
ORDER BY candidate.updated_at, candidate.candidate_id
LIMIT ?
"#,
                [account_id.into(), limit.into()],
            ))
            .await
            .map_err(store_error)?;
        let invalidated = result.rows_affected();
        if invalidated > 0 {
            tracing::debug!(
                account = account.account_id,
                invalidated,
                "来源已失效的 proposed 记忆候选已置为 invalidated"
            );
        }
        Ok(invalidated)
    }

    async fn list_candidates(
        &self,
        account: &SourceAccountRef,
        status: Option<MemoryCandidateStatus>,
        kind: Option<MemoryCandidateKind>,
        limit: u32,
    ) -> Result<Vec<MemoryCandidateView>, InboundEventStoreError> {
        if !(1..=100).contains(&limit) {
            return Err(InboundEventStoreError::InvalidData(
                "memory candidate list limit must be in 1..=100".into(),
            ));
        }
        let account_id = super::mysql_retriever::resolve_account_id(&self.db, account).await?;
        let mut sql = String::from(
            r#"
SELECT candidate.candidate_id, candidate.candidate_kind, candidate.subject_key,
       candidate.candidate_status, candidate.candidate_version,
       CAST(candidate.payload_json AS CHAR) AS payload_json,
       EXISTS (
           SELECT 1 FROM secretary_memory_facts fact
           WHERE fact.account_id = candidate.account_id
             AND fact.fact_kind = candidate.candidate_kind
             AND fact.subject_key = candidate.subject_key
             AND fact.fact_status IN ('proposed', 'confirmed')
       ) AS conflicts_with_active_fact
FROM secretary_memory_candidates candidate
WHERE candidate.account_id = ?
  AND EXISTS (
      SELECT 1 FROM secretary_memory_candidate_sources source0
      WHERE source0.candidate_id = candidate.candidate_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM secretary_memory_candidate_sources source
      LEFT JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
      LEFT JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
      LEFT JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
      LEFT JOIN secretary_message_tombstones tombstone
        ON tombstone.source_event_id = event.source_event_id
       AND tombstone.account_id = event.account_id AND tombstone.status = 'applied'
      WHERE source.candidate_id = candidate.candidate_id
        AND (event.source_event_id IS NULL OR event.account_id <> candidate.account_id
             OR conversation.memory_mode <> 'normal'
             OR content.source_event_id IS NULL OR content.content_mode <> 'normal'
             OR tombstone.source_event_id IS NOT NULL)
  )
"#,
        );
        let mut params: Vec<sea_orm::Value> = vec![account_id.into()];
        if let Some(status) = status {
            sql.push_str(" AND candidate.candidate_status = ?");
            params.push(status.as_str().into());
        }
        if let Some(kind) = kind {
            sql.push_str(" AND candidate.candidate_kind = ?");
            params.push(kind.as_str().into());
        }
        sql.push_str(" ORDER BY candidate.updated_at DESC, candidate.candidate_id DESC LIMIT ?");
        params.push(limit.into());

        let rows = CandidateListRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            &sql,
            params,
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        let mut views = Vec::with_capacity(rows.len());
        for row in rows {
            let candidate_id = MemoryCandidateId::new(row.candidate_id.clone())
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
            let payload: crate::MemoryPayload =
                serde_json::from_str(&row.payload_json).map_err(|error| {
                    InboundEventStoreError::InvalidData(format!(
                        "stored candidate payload is invalid: {error}"
                    ))
                })?;
            let source_rows =
                CandidateSourceRow::find_by_statement(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "SELECT source_event_id, actor_platform_id, content_trust_level, \
                 occurred_at_unix_secs FROM secretary_memory_candidate_sources \
                 WHERE candidate_id = ? ORDER BY source_event_id LIMIT 20",
                    [candidate_id.as_str().into()],
                ))
                .all(&self.db)
                .await
                .map_err(store_error)?;
            let source_excerpts = source_rows
                .into_iter()
                .map(|source| {
                    Ok(MemoryCandidateSourceExcerpt {
                        source_event_id: SourceEventId::new(source.source_event_id)?,
                        actor_id: source.actor_platform_id,
                        occurred_at_unix_secs: source.occurred_at_unix_secs,
                        content_trust_level: parse_trust(&source.content_trust_level)?,
                    })
                })
                .collect::<Result<Vec<_>, InboundEventStoreError>>()?;
            views.push(MemoryCandidateView {
                candidate_id,
                kind: parse_kind(&row.candidate_kind)?,
                subject_key: row.subject_key,
                status: parse_status(&row.candidate_status)?,
                version: MemoryCandidateVersion::new(row.candidate_version)
                    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                payload,
                source_excerpts,
                conflicts_with_active_fact: row.conflicts_with_active_fact != 0,
            });
        }
        Ok(views)
    }
}

/// 从批次行构建有界事件列表：单条正文超预算时整条省略，超出部分从整批剩余
/// 字符预算扣减。主领取与延期领取共用，保证两种路径的截断语义一致。
fn build_candidate_events(
    account: &SourceAccountRef,
    rows: Vec<CandidateEventRow>,
    max_event_chars: u32,
    max_total_chars: u32,
) -> Result<Vec<MemoryCandidateEvent>, InboundEventStoreError> {
    let mut remaining_chars = max_total_chars as usize;
    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        let char_count = row.normalized_text.chars().count();
        let content_omitted = char_count > max_event_chars as usize || char_count > remaining_chars;
        let normalized_text = if content_omitted {
            String::new()
        } else {
            remaining_chars -= char_count;
            row.normalized_text
        };
        events.push(MemoryCandidateEvent {
            source_event_id: SourceEventId::new(row.source_event_id)?,
            actor: ThreadActorRef {
                account: account.clone(),
                actor_id: row.actor_platform_id,
                platform_identity_kind: None,
            },
            role: parse_role(&row.message_role)?,
            occurred_at_unix_secs: row.occurred_at_unix_secs,
            content_trust_level: parse_trust(&row.content_trust_level)?,
            normalized_text,
            content_omitted,
        });
    }
    Ok(events)
}

/// 按是否放行 local_only 构造 SQL 信任集合（只在本模块固定调用点使用）。
fn trust_clause(allow_local_only: bool) -> &'static str {
    if allow_local_only {
        "'normal', 'local_only'"
    } else {
        "'normal'"
    }
}

fn parse_conversation_kind(value: &str) -> Result<ConversationKind, InboundEventStoreError> {
    match value {
        "private" => Ok(ConversationKind::Private),
        "group" => Ok(ConversationKind::Group),
        "owner_control" => Ok(ConversationKind::OwnerControl),
        other => Err(InboundEventStoreError::InvalidData(format!(
            "unknown conversation kind {other}"
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

fn parse_trust(value: &str) -> Result<ContentTrustLevel, InboundEventStoreError> {
    match value {
        "normal" => Ok(ContentTrustLevel::Normal),
        "local_only" => Ok(ContentTrustLevel::LocalOnly),
        "envelope_only" => Ok(ContentTrustLevel::EnvelopeOnly),
        "never_long_term" => Ok(ContentTrustLevel::NeverLongTerm),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown content trust level {value}"
        ))),
    }
}

fn parse_kind(value: &str) -> Result<MemoryCandidateKind, InboundEventStoreError> {
    match value {
        "person" => Ok(MemoryCandidateKind::Person),
        "project" => Ok(MemoryCandidateKind::Project),
        "commitment" => Ok(MemoryCandidateKind::Commitment),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown candidate kind {value}"
        ))),
    }
}

fn parse_status(value: &str) -> Result<MemoryCandidateStatus, InboundEventStoreError> {
    match value {
        "proposed" => Ok(MemoryCandidateStatus::Proposed),
        "approved" => Ok(MemoryCandidateStatus::Approved),
        "rejected" => Ok(MemoryCandidateStatus::Rejected),
        "invalidated" => Ok(MemoryCandidateStatus::Invalidated),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown candidate status {value}"
        ))),
    }
}

#[derive(Debug, FromQueryResult)]
struct FirstConversationRow {
    conversation_id: u64,
    conversation_kind: String,
    platform_conversation_id: String,
}

/// 会话切换点：全局排序中第一个不属于本批会话的可提取事件（边界，不含本身）。
#[derive(Debug, FromQueryResult)]
struct SwitchPointRow {
    source_event_id: String,
    received_at: NaiveDateTime,
}

#[derive(Debug, FromQueryResult)]
struct CandidateStateRow {
    account_id: u64,
    last_received_at: Option<NaiveDateTime>,
    last_source_event_id: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct CandidateEventRow {
    source_event_id: String,
    actor_platform_id: String,
    message_role: String,
    occurred_at_unix_secs: i64,
    received_at: NaiveDateTime,
    content_trust_level: String,
    normalized_text: String,
}

#[derive(Debug, FromQueryResult)]
struct StateCountRow {
    value: i64,
}

/// 指纹存在性探测行：`SELECT 1 AS value` 在 MySQL 中返回 BIGINT，行模型必须匹配。
#[derive(Debug, FromQueryResult)]
struct FingerprintRow {
    #[allow(dead_code)]
    value: i64,
}

#[derive(Debug, FromQueryResult)]
struct CandidateListRow {
    candidate_id: String,
    candidate_kind: String,
    subject_key: String,
    candidate_status: String,
    candidate_version: u64,
    payload_json: String,
    conflicts_with_active_fact: i64,
}

#[derive(Debug, FromQueryResult)]
struct CandidateSourceRow {
    source_event_id: String,
    actor_platform_id: String,
    content_trust_level: String,
    occurred_at_unix_secs: i64,
}
