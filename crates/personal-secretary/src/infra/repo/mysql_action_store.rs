//! MySQL Action 仓储。实现 [`ActionStoreT`]。
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

use super::mysql_inbound::store_error;
use crate::{
    ActionLeaseToken, ActionRunId, ActionRunSeed, ActionStoreError, ActionStoreT, ClaimedActionRun,
    OwnerResponseDraft, RecentEventRef, SecretaryActionEffect, SecretaryActionReceipt,
    SourceAccountRef, SourceEventId, SuspendedRunClaim,
};

pub(crate) struct MySqlActionStore {
    db: DatabaseConnection,
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
        let now = Utc::now().naive_utc();
        let recent_json = serde_json::to_string(&seed.recent_events)
            .map_err(|e| ActionStoreError::InvalidData(e.to_string()))?;
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_action_runs
                   (run_id, account_id, command_source_event_id, command_text, conversation_id,
                    occurred_at_unix_secs, timezone_offset_secs, recent_events_json,
                    status, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)"#,
                [
                    run_id.as_str().into(),
                    account_id.into(),
                    seed.command_source_event_id.as_str().into(),
                    seed.command_text.clone().into(),
                    seed.conversation_id.clone().into(),
                    seed.occurred_at_unix_secs.into(),
                    seed.timezone_offset_secs.into(),
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
                "SELECT run_id FROM secretary_action_runs WHERE account_id = ? AND command_source_event_id = ? AND planner_version = 'v1'",
                vec![account_id.into(), seed.command_source_event_id.as_str().into()],
            ))
            .one(&self.db)
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

        // CAS 领取：status='pending' AND (next_eligible_at IS NULL OR next_eligible_at <= NOW(6))
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_action_runs
                   SET status = 'running', worker_id = ?, lease_token = ?, lease_expires_at = ?,
                       updated_at = ?
                   WHERE status = 'pending'
                     AND (next_eligible_at IS NULL OR next_eligible_at <= ?)
                   ORDER BY created_at ASC
                   LIMIT 1"#,
                [
                    worker_id.into(),
                    lease_token.as_str().into(),
                    lease_expires.into(),
                    now.into(),
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
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"UPDATE secretary_action_runs
                   SET status = 'running', worker_id = ?, lease_token = ?, lease_expires_at = ?,
                       updated_at = ?
                   WHERE run_id = ? AND status = 'suspended'
                     AND command_source_event_id = ?
                     AND JSON_UNQUOTE(JSON_EXTRACT(last_checkpoint_json, '$.checkpoint_id')) = ?
                     AND JSON_UNQUOTE(JSON_EXTRACT(last_checkpoint_json, '$.proposal_id')) = ?"#,
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
        let row = CheckpointRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT CAST(last_checkpoint_json AS CHAR) AS last_checkpoint_json
               FROM secretary_action_runs
               WHERE run_id = ? AND lease_token = ?"#,
            vec![run_id.as_str().into(), lease_token.as_str().into()],
        ))
        .one(&self.db)
        .await
        .map_err(store_error)?;
        Ok(row.and_then(|r| r.last_checkpoint_json))
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

async fn resolve_account_id(
    db: &DatabaseConnection,
    account: &SourceAccountRef,
) -> Result<u64, ActionStoreError> {
    AccountIdRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ? AND status = 'active'",
        [
            account.channel.as_str().into(),
            account.account_id.clone().into(),
        ],
    ))
    .one(db)
    .await
    .map_err(store_error)?
    .map(|r| r.id)
    .ok_or_else(|| {
        ActionStoreError::InvalidData(format!(
            "account not found: {}/{})",
            account.channel.as_str(),
            account.account_id
        ))
    })
}

fn map_claimed_row(
    row: ClaimedRunRow,
    lease_token: ActionLeaseToken,
) -> Result<Option<ClaimedActionRun>, ActionStoreError> {
    let recent_events: Vec<RecentEventRef> = row
        .recent_events_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let channel = match row.source_channel.as_str() {
        "napcat" => crate::MessageSource::NapCat,
        "qq_open_platform" => crate::MessageSource::QqOpenPlatform,
        other => {
            return Err(ActionStoreError::InvalidData(format!(
                "unknown source_channel: {other}"
            )));
        }
    };
    let account = SourceAccountRef::new(channel, &row.platform_account_id)
        .map_err(|e| ActionStoreError::InvalidData(e.to_string()))?;
    Ok(Some(ClaimedActionRun {
        run_id: ActionRunId::new(&row.run_id)?,
        lease_token,
        account,
        command_source_event_id: SourceEventId::new(&row.command_source_event_id)?,
        command_text: row.command_text,
        conversation_id: row.conversation_id,
        occurred_at_unix_secs: row.occurred_at_unix_secs,
        timezone_offset_secs: row.timezone_offset_secs,
        recent_events,
    }))
}

#[derive(Debug, FromQueryResult)]
struct AccountIdRow {
    id: u64,
}

#[derive(Debug, FromQueryResult)]
struct ExistingRunRow {
    run_id: String,
}

#[allow(dead_code)]
#[derive(Debug, FromQueryResult)]
struct ClaimedRunRow {
    run_id: String,
    account_id: u64,
    command_source_event_id: String,
    command_text: String,
    conversation_id: String,
    occurred_at_unix_secs: i64,
    timezone_offset_secs: i64,
    recent_events_json: Option<String>,
    lease_token: String,
    source_channel: String,
    platform_account_id: String,
}

#[derive(Debug, FromQueryResult)]
struct CheckpointRow {
    last_checkpoint_json: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct EffectReceiptRow {
    proposal_json: String,
    result_ref: String,
}

async fn load_effect_receipt_from<C>(
    connection: &C,
    run_id: &ActionRunId,
    effect_id: &str,
) -> Result<Option<SecretaryActionReceipt>, ActionStoreError>
where
    C: ConnectionTrait,
{
    let row = EffectReceiptRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT CAST(proposal_json AS CHAR) AS proposal_json, result_ref FROM secretary_action_effect_receipts WHERE run_id = ? AND effect_id = ?",
        vec![run_id.as_str().into(), effect_id.into()],
    ))
    .one(connection)
    .await
    .map_err(store_error)?;
    row.map(|row| {
        let proposal: crate::SecretaryActionProposal = serde_json::from_str(&row.proposal_json)
            .map_err(|error| ActionStoreError::InvalidData(error.to_string()))?;
        Ok(SecretaryActionReceipt {
            proposal_id: proposal.proposal_id,
            result_ref: row.result_ref,
        })
    })
    .transpose()
}

// ===== P0-4: MySQL CheckpointStore（保存完整 AgentCheckpoint，CAS 单次消费）=====
//
// P0 修复：checkpoint.run_id() 是 Agent Core 内部生成的 RunId，不是业务 action_run_id。
// 用 BoundActionCheckpointStore 绑定业务 ActionRunId，save 时用它作为外键，
// 避免 FK 违约。

use crate::SecretaryAgentState;
use agent_core::graph::{AgentCheckpoint, CheckpointError, CheckpointId, CheckpointStore};

/// 绑定业务 ActionRunId 的 CheckpointStore。
/// Graph 内部的 run_id（checkpoint.run_id()）与业务 run_id 不同；
/// 此包装器确保数据库外键引用的是 secretary_action_runs.run_id。
pub(crate) struct BoundActionCheckpointStore {
    inner: MySqlActionStore,
    action_run_id: ActionRunId,
}

impl BoundActionCheckpointStore {
    pub(crate) fn new(db: DatabaseConnection, action_run_id: ActionRunId) -> Self {
        Self {
            inner: MySqlActionStore::new(db),
            action_run_id,
        }
    }
}

#[async_trait]
impl CheckpointStore<SecretaryAgentState> for BoundActionCheckpointStore {
    async fn save(
        &self,
        checkpoint: AgentCheckpoint<SecretaryAgentState>,
    ) -> Result<(), CheckpointError> {
        let checkpoint_id = checkpoint.id();
        // P0 修复：用业务 action_run_id 而非 checkpoint.run_id()（Graph 内部 RunId）。
        let action_run_id = self.action_run_id.as_str();
        let checkpoint_json = serde_json::to_string(&checkpoint).map_err(|error| {
            tracing::error!(%error, %checkpoint_id, "failed to serialize action checkpoint");
            CheckpointError::StoreUnavailable
        })?;
        let result = self
            .inner
            .db_ref()
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT IGNORE INTO secretary_action_checkpoints
                   (checkpoint_id, run_id, checkpoint_json, checkpoint_status)
                   VALUES (?, ?, ?, 'active')"#,
                vec![
                    checkpoint_id.to_string().into(),
                    action_run_id.into(),
                    checkpoint_json.into(),
                ],
            ))
            .await
            .map_err(|e| checkpoint_store_error(e, checkpoint_id))?;
        if result.rows_affected() != 1 {
            return Err(CheckpointError::Duplicate { checkpoint_id });
        }
        debug!(%checkpoint_id, action_run_id, "action checkpoint persisted");
        Ok(())
    }

    async fn load(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<AgentCheckpoint<SecretaryAgentState>, CheckpointError> {
        let row = FullCheckpointRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(checkpoint_json AS CHAR) AS checkpoint_json FROM secretary_action_checkpoints WHERE checkpoint_id = ? AND checkpoint_status = 'active'",
            vec![checkpoint_id.to_string().into()],
        ))
        .one(self.inner.db_ref())
        .await
        .map_err(|e| checkpoint_store_error(e, checkpoint_id))?
        .ok_or(CheckpointError::NotFound { checkpoint_id })?;
        deserialize_checkpoint(&row.checkpoint_json, checkpoint_id)
    }

    async fn take(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<AgentCheckpoint<SecretaryAgentState>, CheckpointError> {
        let transaction = self
            .inner
            .db_ref()
            .begin()
            .await
            .map_err(|e| checkpoint_store_error(e, checkpoint_id))?;
        let row = FullCheckpointRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(checkpoint_json AS CHAR) AS checkpoint_json FROM secretary_action_checkpoints WHERE checkpoint_id = ? AND checkpoint_status = 'active' FOR UPDATE",
            vec![checkpoint_id.to_string().into()],
        ))
        .one(&transaction)
        .await
        .map_err(|e| checkpoint_store_error(e, checkpoint_id))?
        .ok_or(CheckpointError::NotFound { checkpoint_id })?;
        let checkpoint = deserialize_checkpoint(&row.checkpoint_json, checkpoint_id)?;
        let result = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_action_checkpoints SET checkpoint_status = 'consumed', consumed_at = ? WHERE checkpoint_id = ? AND checkpoint_status = 'active'",
                vec![Utc::now().naive_utc().into(), checkpoint_id.to_string().into()],
            ))
            .await
            .map_err(|e| checkpoint_store_error(e, checkpoint_id))?;
        if result.rows_affected() != 1 {
            return Err(CheckpointError::NotFound { checkpoint_id });
        }
        transaction
            .commit()
            .await
            .map_err(|e| checkpoint_store_error(e, checkpoint_id))?;
        debug!(%checkpoint_id, "action checkpoint consumed");
        Ok(checkpoint)
    }
}

fn deserialize_checkpoint(
    checkpoint_json: &str,
    checkpoint_id: CheckpointId,
) -> Result<AgentCheckpoint<SecretaryAgentState>, CheckpointError> {
    let checkpoint = serde_json::from_str::<AgentCheckpoint<SecretaryAgentState>>(checkpoint_json)
        .map_err(|error| {
            tracing::error!(%error, %checkpoint_id, "failed to deserialize action checkpoint");
            CheckpointError::StoreUnavailable
        })?;
    if checkpoint.id() != checkpoint_id {
        tracing::error!(
            %checkpoint_id,
            stored_checkpoint_id = %checkpoint.id(),
            "action checkpoint identity mismatch"
        );
        return Err(CheckpointError::StoreUnavailable);
    }
    Ok(checkpoint)
}

fn checkpoint_store_error(error: sea_orm::DbErr, checkpoint_id: CheckpointId) -> CheckpointError {
    tracing::error!(%error, %checkpoint_id, "action checkpoint store operation failed");
    CheckpointError::StoreUnavailable
}

#[derive(Debug, FromQueryResult)]
struct FullCheckpointRow {
    checkpoint_json: String,
}
