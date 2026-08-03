//! MySQL Action 仓储实现 [`ActionStoreT`]。
//!
//! 约束 3：领取用 CAS（status='pending' AND next_eligible_at IS NULL OR <= NOW(6)），
//! RowsAffected==1 检查；所有进度提交验证 lease_token；退避时间在 Rust 中饱和计算。
//! 约束 8：错误分类不能全映射为 UnknownCommit。

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use tracing::{debug, info, warn};

use super::super::mysql_inbound::store_error;
use super::queries::{
    CheckpointRow, ClaimedRunRow, ExistingRunRow, load_effect_receipt_from, map_claimed_row,
    resolve_account_id,
};
use crate::{
    ActionLeaseToken, ActionRunId, ActionRunSeed, ActionStoreError, ActionStoreT, ClaimedActionRun,
    OwnerResponseDraft, SecretaryActionEffect, SecretaryActionReceipt, SuspendedRunClaim,
};

pub(crate) struct MySqlActionStore {
    db: DatabaseConnection,
}

#[derive(Debug, FromQueryResult)]
struct SuspendedRunRow {
    run_id: String,
    checkpoint_id: String,
    proposal_id: String,
    command_source_event_id: String,
}

impl MySqlActionStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// 供 BoundActionCheckpointStore 访问底层连接。
    pub(crate) fn db_ref(&self) -> &DatabaseConnection {
        &self.db
    }
}

#[async_trait]
impl ActionStoreT for MySqlActionStore {
    async fn ensure_action_run(
        &self,
        run_id: &ActionRunId,
        seed: &ActionRunSeed,
    ) -> Result<bool, ActionStoreError> {
        let account_id = resolve_account_id(&self.db, &seed.account).await?;
        // 幂等重放只返回既有 Run，不需要重新授予已经发生过的创建动作。
        if let Some(existing) = ExistingRunRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT run_id FROM secretary_action_runs WHERE account_id = ? AND command_source_event_id = ? AND planner_version = 'v1'",
            vec![account_id.into(), seed.command_source_event_id.as_str().into()],
        ))
        .one(&self.db)
        .await
        .map_err(store_error)?
        {
            debug!(
                requested_run_id = run_id.as_str(),
                existing_run_id = existing.run_id,
                "action run already exists"
            );
            return Ok(false);
        }
        let now = Utc::now().naive_utc();
        let recent_json = serde_json::to_string(&seed.recent_events)
            .map_err(|e| ActionStoreError::InvalidData(e.to_string()))?;
        let transaction = self.db.begin().await.map_err(store_error)?;
        // CMD-010 防线 A：创建本身也必须由权威 OwnerCommand 授权，不能先插入
        // 任意 ActionRun 再依赖 Worker 领取时过滤。
        super::super::owner_authorization::verify_owner_command(
            &transaction,
            &seed.command_source_event_id,
            account_id,
        )
        .await
        .map_err(|error| match error {
            super::super::owner_authorization::OwnerAuthError::Unauthorized => {
                ActionStoreError::InvalidData(
                    "action run creation is not authorized by an OwnerCommand".into(),
                )
            }
            super::super::owner_authorization::OwnerAuthError::Database => {
                ActionStoreError::Database("action run authorization query failed".into())
            }
        })?;
        let result = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_action_runs
                   (run_id, account_id, command_source_event_id, command_text, conversation_id,
                    occurred_at_unix_secs, timezone_offset_secs, timezone_name, recent_events_json,
                    status, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)"#,
                [
                    run_id.as_str().into(),
                    account_id.into(),
                    seed.command_source_event_id.as_str().into(),
                    seed.command_text.clone().into(),
                    seed.conversation_id.clone().into(),
                    seed.occurred_at_unix_secs.into(),
                    seed.timezone_offset_secs.into(),
                    seed.timezone.clone().into(),
                    recent_json.into(),
                    now.into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        let created = result.rows_affected() == 1;
        if created {
            info!(run_id = run_id.as_str(), "action run created");
        } else {
            let existing = ExistingRunRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT run_id FROM secretary_action_runs WHERE account_id = ? AND command_source_event_id = ? AND planner_version = 'v1' FOR UPDATE",
                vec![account_id.into(), seed.command_source_event_id.as_str().into()],
            ))
            .one(&transaction)
            .await
            .map_err(store_error)?
            .ok_or_else(|| {
                ActionStoreError::Database(
                    "INSERT IGNORE inserted no action run and no business-key row exists".into(),
                )
            })?;
            debug!(
                requested_run_id = run_id.as_str(),
                existing_run_id = existing.run_id,
                "action run already exists"
            );
        }
        transaction.commit().await.map_err(store_error)?;
        Ok(created)
    }

    async fn claim_pending_run(
        &self,
        worker_id: &str,
        lease_secs: u64,
        _now_unix_secs: i64,
    ) -> Result<Option<ClaimedActionRun>, ActionStoreError> {
        let now = Utc::now().naive_utc();
        // P0 修复：先回收过期租约。status='running' 且 lease_expires_at < NOW 的 run
        // 重置为 pending，让其他 Worker 可重新领取。增加 attempt 计数用于退避。
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_action_runs
                   SET status = 'pending', lease_token = NULL, lease_expires_at = NULL,
                       worker_id = NULL, last_error = 'lease expired',
                       next_eligible_at = ?, attempt = attempt + 1, updated_at = ?
                   WHERE status = 'running' AND lease_expires_at IS NOT NULL
                     AND lease_expires_at < ?"#,
                [now.into(), now.into(), now.into()],
            ))
            .await
            .map_err(store_error)?;
        let lease_token = ActionLeaseToken::generate();
        let lease_secs = i64::try_from(lease_secs)
            .map_err(|_| ActionStoreError::InvalidData("lease_secs exceeds i64".into()))?;
        let lease_expires = now + chrono::Duration::seconds(lease_secs);

        // CAS 领取：status='pending' AND (next_eligible_at IS NULL OR next_eligible_at <= NOW(6))。
        // CMD-010 防线 A：领取（Worker 处理）必须复验命令事件仍是权威
        // OwnerCommand（message_role + actor_kind）且 active OwnerBinding 仍
        // 匹配；binding 被撤销/替换的 run 不会被领取，从而不产生任何 Effect、
        // 业务状态或 Receipt 副作用（由 Effect 事务层授权兜底）。
        // 多表 UPDATE 不支持 ORDER BY（MySQL 1221），先用派生表按 created_at
        // ASC 选出 FIFO 目标再更新（保留原有领取顺序语义）。
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_action_runs
                   INNER JOIN (
                       SELECT r.run_id
                       FROM secretary_action_runs r
                       INNER JOIN secretary_source_events cmd
                         ON cmd.source_event_id = r.command_source_event_id
                       INNER JOIN secretary_accounts command_account
                         ON command_account.id = cmd.account_id
                       INNER JOIN secretary_conversations command_conversation
                         ON command_conversation.id = cmd.conversation_id
                       INNER JOIN secretary_owner_bindings b
                         ON b.managed_account_id = r.account_id
                        AND b.command_account_id = cmd.account_id
                        AND b.owner_actor_id = cmd.actor_platform_id
                        AND b.status = 'active'
                       WHERE r.status = 'pending'
                         AND (r.next_eligible_at IS NULL OR r.next_eligible_at <= ?)
                         AND cmd.message_role = 'owner_command'
                         AND cmd.actor_kind = 'owner'
                         AND cmd.event_type = 'message'
                         AND cmd.source_channel = 'qq_open_platform'
                         AND command_account.source_channel = 'qq_open_platform'
                         AND command_conversation.conversation_kind = 'owner_control'
                         AND 1 = (
                             SELECT COUNT(*) FROM secretary_owner_bindings active_binding
                             WHERE active_binding.managed_account_id = r.account_id
                               AND active_binding.status = 'active'
                         )
                       ORDER BY r.created_at ASC
                       LIMIT 1
                   ) AS pick ON pick.run_id = secretary_action_runs.run_id
                   SET status = 'running', worker_id = ?, lease_token = ?,
                       lease_expires_at = ?, updated_at = ?"#,
                [
                    now.into(),
                    worker_id.into(),
                    lease_token.as_str().into(),
                    lease_expires.into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            return Ok(None);
        }
        // 查领取到的行（JOIN accounts 获取 channel 和 platform_account_id）。
        // P0 修复：recent_events_json 是 MySQL JSON 列，必须 CAST(... AS CHAR) 才能用 String 解码。
        let row = ClaimedRunRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT r.run_id, r.account_id, r.command_source_event_id, r.command_text,
                      r.conversation_id, r.occurred_at_unix_secs, r.timezone_offset_secs,
                      r.timezone_name AS timezone,
                      CAST(r.recent_events_json AS CHAR) AS recent_events_json,
                      r.lease_token,
                      a.source_channel, a.platform_account_id
               FROM secretary_action_runs r
               INNER JOIN secretary_accounts a ON r.account_id = a.id
               WHERE r.lease_token = ? AND r.status = 'running'"#,
            vec![lease_token.as_str().into()],
        ))
        .one(&self.db)
        .await
        .map_err(store_error)?
        .ok_or(ActionStoreError::LeaseLost)?;
        map_claimed_row(row, lease_token)
    }

    async fn claim_suspended_run(
        &self,
        claim: &SuspendedRunClaim,
    ) -> Result<Option<ClaimedActionRun>, ActionStoreError> {
        let now = chrono::DateTime::<Utc>::from_timestamp(claim.now_unix_secs, 0)
            .ok_or_else(|| ActionStoreError::InvalidData("invalid resume claim timestamp".into()))?
            .naive_utc();
        let lease_secs = i64::try_from(claim.lease_secs)
            .map_err(|_| ActionStoreError::InvalidData("lease_secs exceeds i64".into()))?;
        let lease_token = ActionLeaseToken::generate();
        let lease_expires = now + chrono::Duration::seconds(lease_secs);
        // CMD-010 防线 A：Resume（Suspend 后恢复）同样复验命令事件与 active
        // OwnerBinding。审批后、Effect 提交前 OwnerBinding 被撤销或替换时，
        // resume 不领取（返回 LeaseLost），不修改业务状态、不写成功 Receipt、
        // 不返回乐观成功。
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_action_runs r
                   INNER JOIN secretary_source_events cmd
                     ON cmd.source_event_id = r.command_source_event_id
                   INNER JOIN secretary_accounts command_account
                     ON command_account.id = cmd.account_id
                   INNER JOIN secretary_conversations command_conversation
                     ON command_conversation.id = cmd.conversation_id
                   INNER JOIN secretary_owner_bindings b
                     ON b.managed_account_id = r.account_id
                    AND b.command_account_id = cmd.account_id
                    AND b.owner_actor_id = cmd.actor_platform_id
                    AND b.status = 'active'
                   SET r.status = 'running', r.worker_id = ?, r.lease_token = ?,
                       r.lease_expires_at = ?, r.updated_at = ?
                   WHERE r.run_id = ? AND r.status = 'suspended'
                     AND r.command_source_event_id = ?
                     AND JSON_UNQUOTE(JSON_EXTRACT(r.last_checkpoint_json, '$.checkpoint_id')) = ?
                     AND JSON_UNQUOTE(JSON_EXTRACT(r.last_checkpoint_json, '$.proposal_id')) = ?
                     AND cmd.message_role = 'owner_command'
                     AND cmd.actor_kind = 'owner'
                     AND cmd.event_type = 'message'
                     AND cmd.source_channel = 'qq_open_platform'
                     AND command_account.source_channel = 'qq_open_platform'
                     AND command_conversation.conversation_kind = 'owner_control'
                     AND 1 = (
                         SELECT COUNT(*) FROM secretary_owner_bindings active_binding
                         WHERE active_binding.managed_account_id = r.account_id
                           AND active_binding.status = 'active'
                     )"#,
                [
                    claim.worker_id.clone().into(),
                    lease_token.as_str().into(),
                    lease_expires.into(),
                    now.into(),
                    claim.run_id.as_str().into(),
                    claim.command_source_event_id.as_str().into(),
                    claim.checkpoint_id.clone().into(),
                    claim.proposal_id.clone().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            return Ok(None);
        }
        let row = ClaimedRunRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT r.run_id, r.account_id, r.command_source_event_id, r.command_text,
                      r.conversation_id, r.occurred_at_unix_secs, r.timezone_offset_secs,
                      r.timezone_name AS timezone,
                      CAST(r.recent_events_json AS CHAR) AS recent_events_json,
                      r.lease_token,
                      a.source_channel, a.platform_account_id
               FROM secretary_action_runs r
               INNER JOIN secretary_accounts a ON r.account_id = a.id
               WHERE r.lease_token = ? AND r.status = 'running'"#,
            vec![lease_token.as_str().into()],
        ))
        .one(&self.db)
        .await
        .map_err(store_error)?
        .ok_or(ActionStoreError::LeaseLost)?;
        map_claimed_row(row, lease_token)
    }

    async fn list_suspended_runs(
        &self,
        account: &crate::SourceAccountRef,
        limit: u32,
    ) -> Result<Vec<crate::SuspendedActionRun>, ActionStoreError> {
        if !(1..=100).contains(&limit) {
            return Err(ActionStoreError::InvalidData(
                "suspended run list limit must be in 1..=100".into(),
            ));
        }
        let rows = SuspendedRunRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT r.run_id,
                      JSON_UNQUOTE(JSON_EXTRACT(r.last_checkpoint_json, '$.checkpoint_id')) AS checkpoint_id,
                      JSON_UNQUOTE(JSON_EXTRACT(r.last_checkpoint_json, '$.proposal_id')) AS proposal_id,
                      r.command_source_event_id
               FROM secretary_action_runs r
               INNER JOIN secretary_accounts a ON a.id = r.account_id
               WHERE r.status = 'suspended'
                 AND a.source_channel = ? AND a.platform_account_id = ?
                 AND JSON_UNQUOTE(JSON_EXTRACT(r.last_checkpoint_json, '$.checkpoint_id')) IS NOT NULL
                 AND JSON_UNQUOTE(JSON_EXTRACT(r.last_checkpoint_json, '$.proposal_id')) IS NOT NULL
               ORDER BY r.updated_at DESC, r.run_id DESC LIMIT ?"#,
            [
                account.channel.as_str().into(),
                account.account_id.clone().into(),
                limit.into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(store_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(crate::SuspendedActionRun {
                    run_id: ActionRunId::new(row.run_id)?,
                    checkpoint_id: row.checkpoint_id,
                    proposal_id: row.proposal_id,
                    command_source_event_id: crate::SourceEventId::new(
                        row.command_source_event_id,
                    )?,
                })
            })
            .collect()
    }

    async fn mark_suspended(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
        checkpoint_json: &str,
    ) -> Result<(), ActionStoreError> {
        let now = Utc::now().naive_utc();
        let update = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_action_runs
                   SET status = 'suspended', last_checkpoint_json = ?, worker_id = NULL,
                       lease_token = NULL, lease_expires_at = NULL, next_eligible_at = NULL,
                       updated_at = ?
                   WHERE run_id = ? AND lease_token = ? AND status = 'running'"#,
                [
                    checkpoint_json.into(),
                    now.into(),
                    run_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if update.rows_affected() != 1 {
            return Err(ActionStoreError::LeaseLost);
        }
        debug!(
            run_id = run_id.as_str(),
            "action run suspended and worker lease released"
        );
        Ok(())
    }

    async fn load_checkpoint(
        &self,
        run_id: &ActionRunId,
    ) -> Result<Option<String>, ActionStoreError> {
        let row = CheckpointRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(last_checkpoint_json AS CHAR) AS last_checkpoint_json FROM secretary_action_runs WHERE run_id = ?",
            vec![run_id.as_str().into()],
        ))
        .one(&self.db)
        .await
        .map_err(store_error)?;
        Ok(row.and_then(|r| r.last_checkpoint_json))
    }

    async fn take_checkpoint(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
    ) -> Result<Option<String>, ActionStoreError> {
        let transaction = self.db.begin().await.map_err(store_error)?;
        let row = CheckpointRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT CAST(last_checkpoint_json AS CHAR) AS last_checkpoint_json
               FROM secretary_action_runs
               WHERE run_id = ? AND lease_token = ? AND status = 'running'
               FOR UPDATE"#,
            vec![run_id.as_str().into(), lease_token.as_str().into()],
        ))
        .one(&transaction)
        .await
        .map_err(store_error)?;
        let Some(checkpoint_json) = row.and_then(|row| row.last_checkpoint_json) else {
            transaction.commit().await.map_err(store_error)?;
            return Ok(None);
        };
        let result = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_action_runs
                   SET last_checkpoint_json = NULL, updated_at = ?
                   WHERE run_id = ? AND lease_token = ? AND status = 'running'
                     AND last_checkpoint_json IS NOT NULL"#,
                [
                    Utc::now().naive_utc().into(),
                    run_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            transaction.rollback().await.map_err(store_error)?;
            return Err(ActionStoreError::LeaseLost);
        }
        transaction.commit().await.map_err(store_error)?;
        Ok(Some(checkpoint_json))
    }

    async fn load_effect_receipt(
        &self,
        run_id: &ActionRunId,
        effect_id: &str,
    ) -> Result<Option<SecretaryActionReceipt>, ActionStoreError> {
        load_effect_receipt_from(&self.db, run_id, effect_id).await
    }

    async fn apply_effect(
        &self,
        run_id: &ActionRunId,
        effect: &SecretaryActionEffect,
        effect_id: &str,
        result_ref: &str,
        lease_token: &ActionLeaseToken,
    ) -> Result<SecretaryActionReceipt, ActionStoreError> {
        let now = Utc::now().naive_utc();
        let proposal_json = serde_json::to_string(&effect.proposal)
            .map_err(|e| ActionStoreError::InvalidData(e.to_string()))?;
        // P0-3/4 修复：使用传入的 run_id 和 result_ref，不再伪造 executed:{effect_id}。
        let persistent_ref = result_ref.to_owned();
        let transaction = self.db.begin().await.map_err(store_error)?;
        // 先在同一事务中验证租约，再提交 Receipt，避免 LeaseLost 后留下伪成功记录。
        let verify = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_action_runs SET updated_at = ?
                   WHERE run_id = ? AND lease_token = ? AND status = 'running'"#,
                [
                    now.into(),
                    run_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if verify.rows_affected() != 1 {
            transaction.rollback().await.map_err(store_error)?;
            return Err(ActionStoreError::LeaseLost);
        }
        let inserted = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_action_effect_receipts
                   (effect_id, run_id, proposal_json, result_ref, created_at)
                   VALUES (?, ?, ?, ?, ?)"#,
                vec![
                    effect_id.into(),
                    run_id.as_str().into(),
                    proposal_json.into(),
                    persistent_ref.clone().into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if inserted.rows_affected() == 0 {
            let existing = load_effect_receipt_from(&transaction, run_id, effect_id)
                .await?
                .ok_or_else(|| {
                    ActionStoreError::InvalidData(format!(
                        "effect_id collision across action runs: {effect_id}"
                    ))
                })?;
            transaction.commit().await.map_err(store_error)?;
            return Ok(existing);
        }
        transaction.commit().await.map_err(store_error)?;
        Ok(SecretaryActionReceipt {
            proposal_id: effect.proposal.proposal_id.clone(),
            result_ref: persistent_ref,
            tool_kind: None,
        })
    }

    async fn mark_completed(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
        response_draft: Option<&OwnerResponseDraft>,
    ) -> Result<(), ActionStoreError> {
        let now = Utc::now().naive_utc();
        let draft_json = response_draft
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| ActionStoreError::InvalidData(e.to_string()))?;
        if draft_json
            .as_ref()
            .is_some_and(|value| value.len() > 65_536)
        {
            return Err(ActionStoreError::InvalidData(
                "response draft exceeds 64 KiB".into(),
            ));
        }
        let transaction = self.db.begin().await.map_err(store_error)?;
        let result = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_action_runs
                   SET status = 'completed', completed_at = ?, response_draft_json = ?,
                       lease_token = NULL, lease_expires_at = NULL, updated_at = ?
                   WHERE run_id = ? AND lease_token = ? AND status = 'running'"#,
                [
                    now.into(),
                    draft_json.clone().into(),
                    now.into(),
                    run_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            transaction.rollback().await.map_err(store_error)?;
            warn!(run_id = run_id.as_str(), "mark_completed CAS failed");
            return Err(ActionStoreError::LeaseLost);
        }
        if let Some(response_json) = draft_json.as_ref() {
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    r#"INSERT INTO secretary_action_responses
                       (response_id, run_id, response_json, serialized_bytes, invalidated, created_at)
                       VALUES (?, ?, ?, ?, FALSE, ?)"#,
                    [
                        uuid::Uuid::new_v4().to_string().into(),
                        run_id.as_str().into(),
                        response_json.clone().into(),
                        u32::try_from(response_json.len())
                            .map_err(|_| {
                                ActionStoreError::InvalidData(
                                    "response draft size exceeds u32".into(),
                                )
                            })?
                            .into(),
                        now.into(),
                    ],
                ))
                .await
                .map_err(store_error)?;
        }
        transaction.commit().await.map_err(store_error)?;
        info!(run_id = run_id.as_str(), "action run completed");
        Ok(())
    }

    async fn mark_failed(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
        error: &str,
        next_eligible_at_unix_secs: i64,
    ) -> Result<(), ActionStoreError> {
        let now = Utc::now().naive_utc();
        let next_eligible = chrono::DateTime::<Utc>::from_timestamp(next_eligible_at_unix_secs, 0)
            .map(|dt| dt.naive_utc())
            .unwrap_or(now);
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_action_runs
                   SET status = 'pending', last_error = ?, next_eligible_at = ?,
                       lease_token = NULL, lease_expires_at = NULL, updated_at = ?
                   WHERE run_id = ? AND lease_token = ? AND status = 'running'"#,
                [
                    error.into(),
                    next_eligible.into(),
                    now.into(),
                    run_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            return Err(ActionStoreError::LeaseLost);
        }
        warn!(
            run_id = run_id.as_str(),
            error = error,
            "action run failed, will retry"
        );
        Ok(())
    }

    async fn release_lease(
        &self,
        run_id: &ActionRunId,
        lease_token: &ActionLeaseToken,
    ) -> Result<(), ActionStoreError> {
        let now = Utc::now().naive_utc();
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_action_runs
                   SET status = 'pending', lease_token = NULL, lease_expires_at = NULL,
                       next_eligible_at = ?, updated_at = ?
                   WHERE run_id = ? AND lease_token = ? AND status = 'running'"#,
                [
                    now.into(),
                    now.into(),
                    run_id.as_str().into(),
                    lease_token.as_str().into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        if result.rows_affected() != 1 {
            return Err(ActionStoreError::LeaseLost);
        }
        info!(run_id = run_id.as_str(), "action run lease released");
        Ok(())
    }

    async fn append_audit(
        &self,
        run_id: &ActionRunId,
        event_kind: &str,
        detail_json: &str,
    ) -> Result<(), ActionStoreError> {
        let now = Utc::now().naive_utc();
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_action_audit
                   (audit_id, run_id, event_kind, detail_json, created_at)
                   VALUES (?, ?, ?, ?, ?)"#,
                [
                    uuid::Uuid::new_v4().to_string().into(),
                    run_id.as_str().into(),
                    event_kind.into(),
                    detail_json.into(),
                    now.into(),
                ],
            ))
            .await
            .map_err(store_error)?;
        Ok(())
    }
}
