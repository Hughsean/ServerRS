//! MySQL 实现历史回补状态仓储端口 [`BackfillStateStoreT`]。
//!
//! 关键不变量：
//! - Gap 领取使用数据库原子状态转换（`uncertain -> backfilling`）+ 租约，不依赖进程内锁；
//! - 仅领取空窗已结束（`gap_ended_at IS NOT NULL`）的 Gap，避免回补尚未结束的离线窗口；
//! - 同一个 Gap 不会被并发领取两次（Gap 状态原子更新 + `FOR UPDATE`）；但证据不足回到
//!   `uncertain` 后可再次领取（运行表 `gap_id` 不设唯一键，每次领取创建新运行）；
//! - 回到 `uncertain` 的 Gap 受 `secretary_gap_reclaim_schedule.next_eligible_at` 退避约束，
//!   防止热循环回补并避免饿死后续 Gap；`KnownScopesComplete` 的 Gap 被挂起（极远未来
//!   `next_eligible_at`），停止自动重试，仅人工重验或能力升级后重新排队；
//! - 崩溃后租约过期的 `backfilling` 运行可通过 `reclaim_expired`（`FOR UPDATE` + CAS）恢复；
//! - 回补边界读 `secretary_gap_boundaries` 快照（Gap 创建时冻结），而非领取时漂移的实时游标；
//! - 历史消息走与实时消息相同的 `insert_message_if_absent` 幂等入口（实现自 mysql_inbound）。

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, Condition, ConnectionTrait,
    DatabaseBackend, DatabaseTransaction, EntityTrait, FromQueryResult, QueryFilter, Set,
    Statement, TransactionTrait, Value,
};
use uuid::Uuid;

use crate::{
    BackfillAnchor, BackfillAnomaly, BackfillCursor, BackfillLease, BackfillLeaseToken,
    BackfillOutcome, BackfillRunId, BackfillScopeStatus, BackfillStateStoreT, ClaimedGap,
    ConversationKind, ConversationRef, HistoryCompleteness, InboundEventStoreError, IngestionGapId,
    IngestionGapStatus, KnownScope, MessageSource, ReclaimPolicy, ScopeProgress, SourceAccountRef,
};

use super::MySqlInboundEventStore;
use super::entities::{
    secretary_backfill_runs, secretary_backfill_scopes, secretary_ingestion_gaps,
};
use super::mysql_inbound::store_error;

#[async_trait]
impl BackfillStateStoreT for MySqlInboundEventStore {
    async fn claim_next_gap(
        &self,
        lease: BackfillLease,
    ) -> Result<Option<ClaimedGap>, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        // 原子领取一个 uncertain 且空窗已结束、且退避已过期的 Gap。
        // - gap_ended_at IS NOT NULL：重连已结束空窗，避免回补尚未结束的离线窗口；
        // - next_eligible_at IS NULL OR <= now：退避已过，防止热循环与饿死；
        // - ORDER BY updated_at ASC：最久未处理的 Gap 优先，避免总是领取同一个不可证 Gap；
        // - FOR UPDATE：同一时刻只有一个事务能领取该 Gap。
        let row = GapClaimRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT g.gap_id, g.account_id, g.connection_epoch_id, \
                    a.source_channel, a.platform_account_id \
             FROM secretary_ingestion_gaps g \
             INNER JOIN secretary_accounts a ON a.id = g.account_id \
             LEFT JOIN secretary_gap_reclaim_schedule r ON r.gap_id = g.gap_id \
             WHERE g.status = 'uncertain' \
               AND g.gap_ended_at IS NOT NULL \
               AND (r.next_eligible_at IS NULL OR r.next_eligible_at <= ?) \
             ORDER BY g.updated_at ASC \
             LIMIT 1 \
             FOR UPDATE",
            [now.into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?;

        let Some(row) = row else {
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        };

        // 在任何状态变更前完成持久化身份到领域类型的校验，避免提交后才发现脏数据而留下
        // 无法由当前进程解释的 backfilling 运行。
        let account = SourceAccountRef {
            channel: source_channel_from_str(&row.source_channel)?,
            account_id: row.platform_account_id.clone(),
        };
        let gap_id = IngestionGapId::new(row.gap_id.clone())
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        let connection_epoch_id = crate::ConnectionEpochId::new(row.connection_epoch_id.clone())
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;

        let lease_expires_at = now + chrono::Duration::seconds(lease.lease_secs as i64);
        // 原子状态转换：uncertain -> backfilling。若并发领取，受影响行数为 0。
        let updated = secretary_ingestion_gaps::Entity::update_many()
            .col_expr(
                secretary_ingestion_gaps::Column::Status,
                Expr::value(IngestionGapStatus::Backfilling.as_str()),
            )
            .col_expr(
                secretary_ingestion_gaps::Column::UpdatedAt,
                Expr::value(now),
            )
            .filter(secretary_ingestion_gaps::Column::GapId.eq(row.gap_id.clone()))
            .filter(
                secretary_ingestion_gaps::Column::Status.eq(IngestionGapStatus::Uncertain.as_str()),
            )
            .exec(&transaction)
            .await
            .map_err(store_error)?;

        if updated.rows_affected == 0 {
            // 并发领取或状态已变：放弃，不影响其它 Gap。
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        }

        let run_id = Uuid::new_v4().to_string();
        let lease_token = Uuid::new_v4().to_string();
        secretary_backfill_runs::Entity::insert(secretary_backfill_runs::ActiveModel {
            backfill_run_id: Set(run_id.clone()),
            gap_id: Set(row.gap_id.clone()),
            account_id: Set(row.account_id),
            connection_epoch_id: Set(row.connection_epoch_id.clone()),
            status: Set("backfilling".into()),
            lease_expires_at: Set(Some(lease_expires_at)),
            completeness: Set(HistoryCompleteness::Unprovable.as_str().into()),
            failure_class: Set(None),
            pages_read: Set(0),
            events_read: Set(0),
            accepted: Set(0),
            duplicates: Set(0),
            budget_exhausted: Set(false),
            anomaly_count: Set(0),
            created_at: Set(now),
            updated_at: Set(now),
            completed_at: Set(None),
        })
        .exec(&transaction)
        .await
        .map_err(store_error)?;

        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO secretary_backfill_leases \
                    (backfill_run_id, lease_token, updated_at) \
                 VALUES (?, ?, ?)",
                [
                    run_id.clone().into(),
                    lease_token.clone().into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;

        transaction.commit().await.map_err(store_error)?;

        let claim = ClaimedGap {
            run_id: BackfillRunId::new(run_id)
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
            lease_token: BackfillLeaseToken::new(lease_token)
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
            gap_id,
            account,
            connection_epoch_id,
            is_resume: false,
        };
        Ok(Some(claim))
    }

    async fn reclaim_expired(
        &self,
        lease: BackfillLease,
        limit: u32,
    ) -> Result<Vec<ClaimedGap>, InboundEventStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();
        let lease_expires_at = now + chrono::Duration::seconds(lease.lease_secs as i64);

        // FOR UPDATE 锁定过期运行，串行化并发 reclaim；提交后其它 reclaim 事务看到新租约，
        // 不再匹配 WHERE 条件，避免多进程同时恢复同一运行。
        let expired = ExpiredRunRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT r.backfill_run_id, r.gap_id, r.account_id, r.connection_epoch_id, \
                    a.source_channel, a.platform_account_id \
             FROM secretary_backfill_runs r \
             INNER JOIN secretary_accounts a ON a.id = r.account_id \
             WHERE r.status = 'backfilling' \
               AND (r.lease_expires_at IS NULL OR r.lease_expires_at < ?) \
             ORDER BY r.updated_at ASC \
             LIMIT ? \
             FOR UPDATE",
            [now.into(), u64::from(limit).into()],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?;

        let mut claims = Vec::new();
        for row in expired {
            let account = SourceAccountRef {
                channel: source_channel_from_str(&row.source_channel)?,
                account_id: row.platform_account_id.clone(),
            };
            let run_id = BackfillRunId::new(row.backfill_run_id.clone())
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
            let gap_id = IngestionGapId::new(row.gap_id.clone())
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
            let connection_epoch_id =
                crate::ConnectionEpochId::new(row.connection_epoch_id.clone())
                    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
            let lease_token = Uuid::new_v4().to_string();
            // CAS 续租：仅当运行仍为 backfilling 且租约确为过期时刷新，防御并发。
            let renewed = secretary_backfill_runs::Entity::update_many()
                .col_expr(
                    secretary_backfill_runs::Column::LeaseExpiresAt,
                    Expr::value(lease_expires_at),
                )
                .col_expr(secretary_backfill_runs::Column::UpdatedAt, Expr::value(now))
                .filter(
                    secretary_backfill_runs::Column::BackfillRunId.eq(row.backfill_run_id.clone()),
                )
                .filter(secretary_backfill_runs::Column::Status.eq("backfilling"))
                .filter(
                    Condition::any()
                        .add(secretary_backfill_runs::Column::LeaseExpiresAt.is_null())
                        .add(secretary_backfill_runs::Column::LeaseExpiresAt.lt(now)),
                )
                .exec(&transaction)
                .await
                .map_err(store_error)?;
            if renewed.rows_affected == 0 {
                continue;
            }

            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "INSERT INTO secretary_backfill_leases \
                        (backfill_run_id, lease_token, updated_at) \
                     VALUES (?, ?, ?) \
                     ON DUPLICATE KEY UPDATE \
                        lease_token = VALUES(lease_token), \
                        updated_at = VALUES(updated_at)",
                    [
                        row.backfill_run_id.clone().into(),
                        lease_token.clone().into(),
                        now.into(),
                    ],
                ))
                .await
                .map_err(store_error)?;

            let claim = ClaimedGap {
                run_id,
                lease_token: BackfillLeaseToken::new(lease_token)
                    .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?,
                gap_id,
                account,
                connection_epoch_id,
                is_resume: true,
            };
            claims.push(claim);
        }

        transaction.commit().await.map_err(store_error)?;
        Ok(claims)
    }

    async fn known_scopes_for_gap(
        &self,
        gap_id: &IngestionGapId,
    ) -> Result<Vec<KnownScope>, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;

        // 取 Gap 所属账号主体，用于构造绑定账号视角的边界游标。
        let account_row = GapAccountRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT a.source_channel, a.platform_account_id \
             FROM secretary_ingestion_gaps g \
             INNER JOIN secretary_accounts a ON a.id = g.account_id \
             WHERE g.gap_id = ?",
            [gap_id.as_str().into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?;

        let Some(account_row) = account_row else {
            transaction.commit().await.map_err(store_error)?;
            return Ok(Vec::new());
        };
        let account = SourceAccountRef {
            channel: source_channel_from_str(&account_row.source_channel)?,
            account_id: account_row.platform_account_id,
        };

        // 边界读 Gap 创建时的快照，而非领取时漂移的实时游标。
        let boundaries = BoundaryRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT conversation_kind, platform_conversation_id, boundary_message_id \
             FROM secretary_gap_boundaries \
             WHERE gap_id = ?",
            [gap_id.as_str().into()],
        ))
        .all(&transaction)
        .await
        .map_err(store_error)?;

        transaction.commit().await.map_err(store_error)?;

        boundaries
            .into_iter()
            .map(|b| {
                let conversation = ConversationRef::new(
                    conversation_kind_from_str(&b.conversation_kind)?,
                    b.platform_conversation_id,
                )
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
                // 边界按平台消息 ID 匹配（用例侧），message_seq 不参与边界身份判定。
                let boundary_cursor = Some(BackfillCursor::new(
                    account.clone(),
                    BackfillAnchor::new(b.boundary_message_id, String::new()),
                ));
                Ok(KnownScope {
                    conversation,
                    boundary_cursor,
                })
            })
            .collect()
    }

    async fn record_scope_progress(
        &self,
        run_id: &BackfillRunId,
        lease_token: &BackfillLeaseToken,
        progress: &ScopeProgress,
    ) -> Result<(), InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();

        // 锁定并校验当前租约所有者。令牌不匹配表示该运行已被其它 Worker 接管，旧持有者
        // 不得续租或覆盖进度。
        let account_id = lock_active_lease(&transaction, run_id, lease_token).await?;
        let renewed = secretary_backfill_runs::Entity::update_many()
            .col_expr(
                secretary_backfill_runs::Column::LeaseExpiresAt,
                Expr::value(now + chrono::Duration::seconds(self.lease_secs as i64)),
            )
            .col_expr(secretary_backfill_runs::Column::UpdatedAt, Expr::value(now))
            .filter(secretary_backfill_runs::Column::BackfillRunId.eq(run_id.as_str()))
            .filter(secretary_backfill_runs::Column::Status.eq("backfilling"))
            .exec(&transaction)
            .await
            .map_err(store_error)?;
        if renewed.rows_affected != 1 {
            return Err(InboundEventStoreError::LeaseLost);
        }

        let scope_key = format!(
            "{}:{}",
            progress.conversation.kind.as_str(),
            progress.conversation.id
        );
        let conversation_id = resolve_conversation_id(
            &transaction,
            account_id,
            &progress.conversation.kind,
            &progress.conversation.id,
        )
        .await?;

        let anomalies_json = serde_json::to_value(&progress.anomalies)
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
        let (last_msg_id, last_msg_seq) = progress
            .last_cursor
            .as_ref()
            .map(|cursor| {
                (
                    Some(cursor.anchor.message_id.clone()),
                    Some(cursor.anchor.message_seq.clone()),
                )
            })
            .unwrap_or((None, None));

        let scope_status = progress.status.as_str().to_owned();
        let active = secretary_backfill_scopes::ActiveModel {
            id: NotSet,
            backfill_run_id: Set(run_id.as_str().to_owned()),
            account_id: Set(account_id),
            conversation_id: Set(conversation_id),
            scope_kind: Set(progress.conversation.kind.as_str().into()),
            scope_key: Set(scope_key),
            status: Set(scope_status),
            last_anchor_message_id: Set(last_msg_id),
            last_anchor_message_seq: Set(last_msg_seq),
            pages_read: Set(progress.pages_read),
            events_read: Set(progress.events_read),
            accepted: Set(progress.accepted),
            duplicates: Set(progress.duplicates),
            reached_boundary: Set(progress.reached_boundary),
            anomalies: Set(if progress.anomalies.is_empty() {
                None
            } else {
                Some(anomalies_json)
            }),
            created_at: Set(now),
            updated_at: Set(now),
        };
        // 幂等 upsert：同一 run + scope_key 只保留最新进度。
        secretary_backfill_scopes::Entity::insert(active)
            .on_conflict(
                OnConflict::columns([
                    secretary_backfill_scopes::Column::BackfillRunId,
                    secretary_backfill_scopes::Column::ScopeKey,
                ])
                .update_columns([
                    secretary_backfill_scopes::Column::Status,
                    secretary_backfill_scopes::Column::LastAnchorMessageId,
                    secretary_backfill_scopes::Column::LastAnchorMessageSeq,
                    secretary_backfill_scopes::Column::PagesRead,
                    secretary_backfill_scopes::Column::EventsRead,
                    secretary_backfill_scopes::Column::Accepted,
                    secretary_backfill_scopes::Column::Duplicates,
                    secretary_backfill_scopes::Column::ReachedBoundary,
                    secretary_backfill_scopes::Column::Anomalies,
                    secretary_backfill_scopes::Column::UpdatedAt,
                ])
                .to_owned(),
            )
            .exec(&transaction)
            .await
            .map_err(store_error)?;

        transaction.commit().await.map_err(store_error)?;
        Ok(())
    }

    async fn load_run_progress(
        &self,
        run_id: &BackfillRunId,
    ) -> Result<Option<Vec<ScopeProgress>>, InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let scopes = secretary_backfill_scopes::Entity::find()
            .filter(secretary_backfill_scopes::Column::BackfillRunId.eq(run_id.as_str()))
            .all(&transaction)
            .await
            .map_err(store_error)?;

        if scopes.is_empty() {
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        }

        let account_ids: std::collections::HashSet<u64> =
            scopes.iter().map(|scope| scope.account_id).collect();
        let mut account_refs: std::collections::HashMap<u64, SourceAccountRef> =
            std::collections::HashMap::new();
        for account_id in account_ids {
            let account = super::entities::secretary_accounts::Entity::find_by_id(account_id)
                .one(&transaction)
                .await
                .map_err(store_error)?
                .ok_or(InboundEventStoreError::Unavailable)?;
            account_refs.insert(
                account_id,
                SourceAccountRef {
                    channel: source_channel_from_str(&account.source_channel)?,
                    account_id: account.platform_account_id,
                },
            );
        }

        let mut progress = Vec::with_capacity(scopes.len());
        for scope in scopes {
            let kind = conversation_kind_from_str(&scope.scope_kind)
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
            let conversation = ConversationRef::new(
                kind,
                resolve_conversation_platform_id_in_txn(&transaction, scope.conversation_id)
                    .await?,
            )
            .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))?;
            let account_ref = account_refs
                .get(&scope.account_id)
                .cloned()
                .ok_or(InboundEventStoreError::Unavailable)?;
            let last_cursor = match (scope.last_anchor_message_id, scope.last_anchor_message_seq) {
                (Some(message_id), message_seq) => Some(BackfillCursor::new(
                    account_ref,
                    BackfillAnchor::new(message_id, message_seq.unwrap_or_default()),
                )),
                _ => None,
            };
            let anomalies: Vec<BackfillAnomaly> = scope
                .anomalies
                .as_ref()
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_default();
            progress.push(ScopeProgress {
                conversation,
                status: BackfillScopeStatus::parse_from_str(&scope.status).ok_or_else(|| {
                    InboundEventStoreError::InvalidData(format!(
                        "unknown scope status: {}",
                        scope.status
                    ))
                })?,
                last_cursor,
                pages_read: scope.pages_read,
                events_read: scope.events_read,
                accepted: scope.accepted,
                duplicates: scope.duplicates,
                reached_boundary: scope.reached_boundary,
                anomalies,
            });
        }
        transaction.commit().await.map_err(store_error)?;
        Ok(Some(progress))
    }

    async fn finalize_run(
        &self,
        outcome: &BackfillOutcome,
        lease_token: &BackfillLeaseToken,
    ) -> Result<(), InboundEventStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let now = Utc::now().naive_utc();

        // 必须先锁定运行并验证租约令牌，再允许修改 Gap。若同一令牌的终态提交已成功，
        // 则把重试视为幂等成功（例如提交成功但响应丢失）；令牌已轮换才是 LeaseLost。
        let owned_run = lock_owned_run(&transaction, &outcome.run_id, lease_token).await?;
        if owned_run.gap_id != outcome.gap_id.as_str() {
            return Err(InboundEventStoreError::InvalidData(format!(
                "backfill run {} belongs to gap {}, not {}",
                outcome.run_id.as_str(),
                owned_run.gap_id,
                outcome.gap_id.as_str()
            )));
        }
        if owned_run.status != "backfilling" {
            let existing = secretary_ingestion_gaps::Entity::find_by_id(outcome.gap_id.as_str())
                .one(&transaction)
                .await
                .map_err(store_error)?
                .ok_or_else(|| {
                    InboundEventStoreError::InvalidData("gap not found on finalize retry".into())
                })?;
            if existing.status == outcome.gap_target_status.as_str()
                && owned_run.completeness == outcome.completeness.as_str()
            {
                transaction.commit().await.map_err(store_error)?;
                return Ok(());
            }
            return Err(InboundEventStoreError::InvalidData(format!(
                "backfill run {} is already {}/{} but gap {} is {}",
                outcome.run_id.as_str(),
                owned_run.status,
                owned_run.completeness,
                outcome.gap_id.as_str(),
                existing.status
            )));
        }

        // 原子更新 Gap 状态：backfilling -> target。并发或重复 finalize 时受影响行数为 0。
        let updated = secretary_ingestion_gaps::Entity::update_many()
            .col_expr(
                secretary_ingestion_gaps::Column::Status,
                Expr::value(outcome.gap_target_status.as_str()),
            )
            .col_expr(
                secretary_ingestion_gaps::Column::UpdatedAt,
                Expr::value(now),
            )
            .filter(secretary_ingestion_gaps::Column::GapId.eq(outcome.gap_id.as_str()))
            .filter(
                secretary_ingestion_gaps::Column::Status
                    .eq(IngestionGapStatus::Backfilling.as_str()),
            )
            .exec(&transaction)
            .await
            .map_err(store_error)?;

        // 若 Gap 已被并发 finalize，仍把运行标记为终态（幂等），但不重复改 Gap。
        if updated.rows_affected == 0 {
            let existing = secretary_ingestion_gaps::Entity::find_by_id(outcome.gap_id.as_str())
                .one(&transaction)
                .await
                .map_err(store_error)?
                .ok_or_else(|| {
                    InboundEventStoreError::InvalidData("gap not found on finalize".into())
                })?;
            if existing.status != outcome.gap_target_status.as_str() {
                return Err(InboundEventStoreError::InvalidData(format!(
                    "gap {} is in terminal state {} and cannot be finalized to {}",
                    outcome.gap_id.as_str(),
                    existing.status,
                    outcome.gap_target_status.as_str()
                )));
            }
        }

        let anomaly_count = outcome
            .evidence
            .scopes
            .iter()
            .map(|scope| scope.anomalies.len() as u32)
            .sum::<u32>();
        let pages_read = outcome
            .evidence
            .scopes
            .iter()
            .map(|scope| scope.pages_read)
            .sum::<u32>();
        let events_read = outcome
            .evidence
            .scopes
            .iter()
            .map(|scope| scope.events_read)
            .sum::<u32>();
        let accepted = outcome
            .evidence
            .scopes
            .iter()
            .map(|scope| scope.accepted)
            .sum::<u32>();
        let duplicates = outcome
            .evidence
            .scopes
            .iter()
            .map(|scope| scope.duplicates)
            .sum::<u32>();

        let run_status = match outcome.completeness {
            HistoryCompleteness::ProvenComplete => "verified_complete",
            HistoryCompleteness::KnownScopesComplete | HistoryCompleteness::Unprovable => {
                "unprovable"
            }
            HistoryCompleteness::Unrecoverable => "unrecoverable",
        };
        let failure_class = if matches!(
            outcome.completeness,
            HistoryCompleteness::KnownScopesComplete | HistoryCompleteness::Unprovable
        ) {
            Some(
                outcome
                    .gap_reason
                    .map(|reason| reason.as_str().to_owned())
                    .unwrap_or_else(|| outcome.completeness.as_str().to_owned()),
            )
        } else {
            None
        };

        let mut active: secretary_backfill_runs::ActiveModel =
            secretary_backfill_runs::Entity::find_by_id(outcome.run_id.as_str())
                .one(&transaction)
                .await
                .map_err(store_error)?
                .ok_or_else(|| {
                    InboundEventStoreError::InvalidData("backfill run not found".into())
                })?
                .into();
        active.status = Set(run_status.into());
        active.completeness = Set(outcome.completeness.as_str().into());
        active.failure_class = Set(failure_class);
        active.pages_read = Set(pages_read);
        active.events_read = Set(events_read);
        active.accepted = Set(accepted);
        active.duplicates = Set(duplicates);
        active.budget_exhausted = Set(outcome.evidence.budget_exhausted);
        active.anomaly_count = Set(anomaly_count);
        active.updated_at = Set(now);
        active.completed_at = Set(Some(now));
        active.update(&transaction).await.map_err(store_error)?;

        // 退避调度：根据完整性判定的 reclaim_policy 操作 reclaim_schedule 表。
        // - Terminal：Gap 已达终态，删除 reclaim_schedule 行。
        // - Backoff(secs)：Gap 保持 uncertain，设置 next_eligible_at = now + secs。
        // - Suspended：Gap 保持 uncertain，设置极远未来 next_eligible_at，停止自动重试。
        //   用于 KnownScopesComplete：Gap 边界已冻结，重跑无新证据。
        match outcome.completeness.reclaim_policy() {
            ReclaimPolicy::Terminal => {
                transaction
                    .execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::MySql,
                        "DELETE FROM secretary_gap_reclaim_schedule WHERE gap_id = ?",
                        [outcome.gap_id.as_str().into()],
                    ))
                    .await
                    .map_err(store_error)?;
            }
            ReclaimPolicy::Backoff(secs) => {
                let next_eligible_at = now + chrono::Duration::seconds(secs as i64);
                let values: Vec<Value> = vec![
                    outcome.gap_id.as_str().into(),
                    next_eligible_at.into(),
                    now.into(),
                ];
                transaction
                    .execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::MySql,
                        "INSERT INTO secretary_gap_reclaim_schedule \
                            (gap_id, next_eligible_at, updated_at) \
                         VALUES (?, ?, ?) \
                         ON DUPLICATE KEY UPDATE \
                            next_eligible_at = VALUES(next_eligible_at), \
                            updated_at = VALUES(updated_at)",
                        values,
                    ))
                    .await
                    .map_err(store_error)?;
            }
            ReclaimPolicy::Suspended => {
                // 设置极远未来时间（9999-12-31），使该 Gap 在自动领取查询中永远不可领取。
                // 人工重验时可直接 DELETE 该行或更新 next_eligible_at。
                let suspended_until = chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(9999, 12, 31).unwrap(),
                    chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
                );
                let values: Vec<Value> = vec![
                    outcome.gap_id.as_str().into(),
                    suspended_until.into(),
                    now.into(),
                ];
                transaction
                    .execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::MySql,
                        "INSERT INTO secretary_gap_reclaim_schedule \
                            (gap_id, next_eligible_at, updated_at) \
                         VALUES (?, ?, ?) \
                         ON DUPLICATE KEY UPDATE \
                            next_eligible_at = VALUES(next_eligible_at), \
                            updated_at = VALUES(updated_at)",
                        values,
                    ))
                    .await
                    .map_err(store_error)?;
            }
        }

        transaction.commit().await.map_err(store_error)?;
        Ok(())
    }
}

/// 锁定并验证一个仍由给定令牌持有的活动运行，返回其账号主键。
async fn lock_active_lease(
    transaction: &DatabaseTransaction,
    run_id: &BackfillRunId,
    lease_token: &BackfillLeaseToken,
) -> Result<u64, InboundEventStoreError> {
    let row = lock_owned_run(transaction, run_id, lease_token).await?;
    if row.status != "backfilling" {
        return Err(InboundEventStoreError::LeaseLost);
    }
    Ok(row.account_id)
}

/// 锁定一个仍由给定令牌持有的运行。终态提交重试也允许读取已终结运行，以保持幂等。
async fn lock_owned_run(
    transaction: &DatabaseTransaction,
    run_id: &BackfillRunId,
    lease_token: &BackfillLeaseToken,
) -> Result<OwnedRunRow, InboundEventStoreError> {
    OwnedRunRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT r.account_id, r.gap_id, r.status, r.completeness \
         FROM secretary_backfill_runs r \
         INNER JOIN secretary_backfill_leases l \
            ON l.backfill_run_id = r.backfill_run_id \
         WHERE r.backfill_run_id = ? \
           AND l.lease_token = ? \
         FOR UPDATE",
        [run_id.as_str().into(), lease_token.as_str().into()],
    ))
    .one(transaction)
    .await
    .map_err(store_error)?
    .ok_or(InboundEventStoreError::LeaseLost)
}

async fn resolve_conversation_id(
    transaction: &DatabaseTransaction,
    account_id: u64,
    kind: &ConversationKind,
    platform_conversation_id: &str,
) -> Result<u64, InboundEventStoreError> {
    use super::entities::secretary_conversations;
    let existing = secretary_conversations::Entity::find()
        .filter(secretary_conversations::Column::AccountId.eq(account_id))
        .filter(secretary_conversations::Column::ConversationKind.eq(kind.as_str()))
        .filter(
            secretary_conversations::Column::PlatformConversationId.eq(platform_conversation_id),
        )
        .one(transaction)
        .await
        .map_err(store_error)?
        .map(|model| model.id);
    existing.ok_or_else(|| {
        InboundEventStoreError::InvalidData(format!(
            "conversation {}/{platform_conversation_id} not known; \
             backfill only targets conversations that have a realtime cursor",
            kind.as_str()
        ))
    })
}

async fn resolve_conversation_platform_id_in_txn(
    transaction: &DatabaseTransaction,
    conversation_id: u64,
) -> Result<String, InboundEventStoreError> {
    use super::entities::secretary_conversations;
    secretary_conversations::Entity::find_by_id(conversation_id)
        .one(transaction)
        .await
        .map_err(store_error)?
        .map(|model| model.platform_conversation_id)
        .ok_or(InboundEventStoreError::Unavailable)
}

fn source_channel_from_str(value: &str) -> Result<MessageSource, InboundEventStoreError> {
    match value {
        "napcat" => Ok(MessageSource::NapCat),
        "qq_open_platform" => Ok(MessageSource::QqOpenPlatform),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown source channel: {value}"
        ))),
    }
}

fn conversation_kind_from_str(value: &str) -> Result<ConversationKind, InboundEventStoreError> {
    match value {
        "private" => Ok(ConversationKind::Private),
        "group" => Ok(ConversationKind::Group),
        "owner_control" => Ok(ConversationKind::OwnerControl),
        _ => Err(InboundEventStoreError::InvalidData(format!(
            "unknown conversation kind: {value}"
        ))),
    }
}

#[derive(FromQueryResult)]
struct GapClaimRow {
    gap_id: String,
    account_id: u64,
    connection_epoch_id: String,
    source_channel: String,
    platform_account_id: String,
}

#[derive(FromQueryResult)]
struct ExpiredRunRow {
    backfill_run_id: String,
    gap_id: String,
    #[allow(dead_code)]
    account_id: u64,
    connection_epoch_id: String,
    source_channel: String,
    platform_account_id: String,
}

#[derive(FromQueryResult)]
struct GapAccountRow {
    source_channel: String,
    platform_account_id: String,
}

#[derive(FromQueryResult)]
struct BoundaryRow {
    conversation_kind: String,
    platform_conversation_id: String,
    boundary_message_id: String,
}

#[derive(FromQueryResult)]
struct OwnedRunRow {
    account_id: u64,
    gap_id: String,
    status: String,
    completeness: String,
}
