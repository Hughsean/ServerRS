//! 延迟 Reply 修复仓储（Codex 复核 P1-1）。
//!
//! 候选集合 = `secretary_source_events` 中 unresolved 子事件
//! （`reply_to_platform_event_id IS NOT NULL AND reply_to_event_id IS NULL`）；
//! `secretary_reply_reconcile_claims` 只为候选提供租约与指数退避，使修复在跨重启、
//! 多 Worker 下安全（fencing 令牌 + SKIP LOCKED + 过期恢复）。

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseBackend, FromQueryResult, Statement, TransactionTrait};
use uuid::Uuid;

use personal_secretary::{
    ClaimedPendingReply, ConversationKind, ConversationRef, InboundEventStoreError,
    ReplyReconcileStoreT, SourceAccountRef, SourceEventId,
};

use super::MySqlInboundEventStore;
use super::mysql_inbound::{resolve_pending_replies_in_txn, resolve_reply_by_refs, store_error};

/// unresolved 候选行（source_events × 退避簿 左连接）。
#[derive(Debug, FromQueryResult)]
struct ReconcileCandidateRow {
    source_event_id: String,
    source_channel: String,
    platform_account_id: String,
    conversation_kind: String,
    platform_conversation_id: String,
    reply_to_platform_event_id: String,
    attempts: u32,
}

/// 候选行所属会话/账号信息（处理事务内重新读取，不信任领取快照）。
#[derive(Debug, FromQueryResult)]
struct ReconcileEventRow {
    account_id: u64,
    conversation_id: u64,
    source_channel: String,
    reply_to_platform_event_id: String,
    reply_to_event_id: Option<String>,
}

#[async_trait]
impl ReplyReconcileStoreT for MySqlInboundEventStore {
    async fn claim_reconcile_batch(
        &self,
        lease_secs: u64,
        limit: u32,
    ) -> Result<Vec<ClaimedPendingReply>, InboundEventStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        // 从候选队列出发（不扫描全部 source_events）：只领取 unresolved 且
        // 无人持有租约且退避已到期的候选，按收到时间最旧优先。
        // FOR UPDATE SKIP LOCKED 保证同一候选同一时刻只被一个 Worker 领取。
        let rows = ReconcileCandidateRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT s.source_event_id,
                      s.source_channel, a.platform_account_id,
                      c2.conversation_kind, c2.platform_conversation_id,
                      s.reply_to_platform_event_id,
                      r.attempts
               FROM secretary_reply_reconcile_claims r
               INNER JOIN secretary_source_events s
                 ON s.source_event_id = r.source_event_id
                 AND s.reply_to_event_id IS NULL
               INNER JOIN secretary_accounts a ON a.id = s.account_id
               INNER JOIN secretary_conversations c2 ON c2.id = s.conversation_id
               WHERE (r.lease_token IS NULL
                       AND (r.next_eligible_at IS NULL OR r.next_eligible_at <= ?))
                  OR r.lease_expires_at < ?
               ORDER BY s.received_at ASC, s.source_event_id ASC
               LIMIT ?
               FOR UPDATE SKIP LOCKED"#,
            [now.into(), now.into(), limit.into()],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?;

        if rows.is_empty() {
            transaction.commit().await.map_err(store_error)?;
            return Ok(Vec::new());
        }
        let lease_token = Uuid::new_v4().to_string();
        let lease_expires_at = now + chrono::Duration::seconds(lease_secs as i64);
        for row in &rows {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"INSERT INTO secretary_reply_reconcile_claims
                       (source_event_id, lease_token, lease_expires_at, attempts, updated_at)
                       VALUES (?, ?, ?, ?, ?)
                       ON DUPLICATE KEY UPDATE
                         lease_token = VALUES(lease_token),
                         lease_expires_at = VALUES(lease_expires_at),
                         next_eligible_at = NULL,
                         updated_at = VALUES(updated_at)"#,
                    [
                        row.source_event_id.clone().into(),
                        lease_token.clone().into(),
                        lease_expires_at.into(),
                        row.attempts.into(),
                        now.into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
        }
        transaction.commit().await.map_err(store_error)?;
        let mut claimed = Vec::with_capacity(rows.len());
        for row in rows {
            claimed.push(ClaimedPendingReply {
                source_event_id: SourceEventId::new(row.source_event_id)
                    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                account: SourceAccountRef::new(
                    source_channel_from_str(&row.source_channel)?,
                    row.platform_account_id,
                )
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                conversation: ConversationRef::new(
                    conversation_kind_from_str(&row.conversation_kind)?,
                    &row.platform_conversation_id,
                )
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                reply_to_platform_message_id: row.reply_to_platform_event_id,
                lease_token: lease_token.clone(),
                attempts: row.attempts,
            });
        }
        Ok(claimed)
    }

    async fn resolve_claimed_pending_reply(
        &self,
        claimed: &ClaimedPendingReply,
        retry_initial_ms: u64,
        retry_max_ms: u64,
    ) -> Result<bool, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        // fencing（Codex 第四轮复核 #1）：锁定退避簿行并复验租约。
        // `query_one_raw` 返回行表示令牌有效且未过期；返回 None 表示令牌不匹配或
        // 已过期——旧 Worker 记录已被覆盖，本轮必须放弃。
        let fence_row = transaction
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT 1 FROM secretary_reply_reconcile_claims \
                 WHERE source_event_id = ? AND lease_token = ? AND lease_expires_at >= ? \
                 FOR UPDATE",
                [
                    claimed.source_event_id.as_str().into(),
                    claimed.lease_token.clone().into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if fence_row.is_none() {
            transaction.commit().await.map_err(store_error)?;
            return Ok(false);
        }
        // 处理前按持久化行重新读取作用域与当前状态（不信任领取快照）。
        let event = ReconcileEventRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT account_id, conversation_id, source_channel,
                      reply_to_platform_event_id, reply_to_event_id
               FROM secretary_source_events
               WHERE source_event_id = ?"#,
            [claimed.source_event_id.as_str().into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?;
        let Some(event) = event else {
            fenced_clear_reconcile_claim(
                &transaction,
                claimed.source_event_id.as_str(),
                &claimed.lease_token,
                &now,
            )
            .await?;
            transaction.commit().await.map_err(store_error)?;
            return Ok(false);
        };
        if event.reply_to_event_id.is_some() {
            fenced_clear_reconcile_claim(
                &transaction,
                claimed.source_event_id.as_str(),
                &claimed.lease_token,
                &now,
            )
            .await?;
            transaction.commit().await.map_err(store_error)?;
            return Ok(false);
        }

        // 与主路径相同的同作用域解析：命中父事件则回填 pending 并失效旧线程投影。
        let parent_event_id = resolve_reply_by_refs(
            &transaction,
            event.account_id,
            event.conversation_id,
            &event.source_channel,
            &event.reply_to_platform_event_id,
        )
        .await?;
        if let Some(parent_event_id) = parent_event_id {
            let resolved = resolve_pending_replies_in_txn(
                &transaction,
                event.account_id,
                event.conversation_id,
                &event.source_channel,
                &event.reply_to_platform_event_id,
                &parent_event_id,
            )
            .await?;
            if resolved > 0 {
                transaction.commit().await.map_err(store_error)?;
                return Ok(true);
            }
        }

        // 父事件仍不可见：指数退避并释放租约（fenced 写入，不覆盖新 Worker 租约）。
        let attempts = claimed.attempts.saturating_add(1);
        let exponent = attempts.saturating_sub(1).min(63);
        let backoff_ms = retry_max_ms.min(retry_initial_ms.saturating_mul(1u64 << exponent));
        let safe_ms = backoff_ms.max(1).min(i64::MAX as u64);
        let next_eligible_at = now + chrono::Duration::milliseconds(safe_ms as i64);
        let backed_off = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_reply_reconcile_claims
                   SET lease_token = NULL, lease_expires_at = NULL,
                       attempts = ?, last_error = 'parent not yet available',
                       next_eligible_at = ?, updated_at = ?
                   WHERE source_event_id = ?
                     AND lease_token = ?
                     AND lease_expires_at >= ?"#,
                [
                    attempts.into(),
                    next_eligible_at.into(),
                    now.into(),
                    claimed.source_event_id.as_str().into(),
                    claimed.lease_token.clone().into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if backed_off.rows_affected() == 0 {
            tracing::warn!(
                error_code = "reconcile_backoff_fence_lost",
                "退避写入时租约已变化，旧 Worker 令牌已被覆盖"
            );
        }
        transaction.commit().await.map_err(store_error)?;
        Ok(false)
    }
}

/// fenced 删除单候选的退避簿行：以 lease_token + 未过期为条件，不覆盖另一
/// Worker 持有的租约。已解析或已消失的事件不必强制删除——无租约的残留行在
/// 主路径解析的联动清理或下一轮领取时自然消失。
async fn fenced_clear_reconcile_claim(
    transaction: &sea_orm::DatabaseTransaction,
    source_event_id: &str,
    lease_token: &str,
    now: &chrono::NaiveDateTime,
) -> Result<(), InboundEventStoreError> {
    let deleted = transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "DELETE FROM secretary_reply_reconcile_claims \
             WHERE source_event_id = ? AND lease_token = ? AND lease_expires_at >= ?",
            [source_event_id.into(), lease_token.into(), (*now).into()],
        ))
        .await
        .map_err(store_error)?;
    // 有效租约预期恰好删除 1 行；0 行表示令牌已被覆盖或租约过期——静默成功会
    // 导致状态漂移（Codex 第四轮复核 #1）。
    if deleted.rows_affected() != 1 {
        return Err(InboundEventStoreError::LeaseLost);
    }
    Ok(())
}

fn source_channel_from_str(
    value: &str,
) -> Result<personal_secretary::MessageSource, InboundEventStoreError> {
    match value {
        "napcat" => Ok(personal_secretary::MessageSource::NapCat),
        "qq_open_platform" => Ok(personal_secretary::MessageSource::QqOpenPlatform),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown source channel {value}"
        ))),
    }
}

fn conversation_kind_from_str(value: &str) -> Result<ConversationKind, InboundEventStoreError> {
    match value {
        "group" => Ok(ConversationKind::Group),
        "private" => Ok(ConversationKind::Private),
        "owner_control" => Ok(ConversationKind::OwnerControl),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown conversation kind {value}"
        ))),
    }
}
