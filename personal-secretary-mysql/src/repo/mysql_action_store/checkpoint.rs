//! 绑定业务 ActionRunId 的 MySQL CheckpointStore（保存完整 AgentCheckpoint，CAS 单次消费）。
//!
//! P0 修复：checkpoint.run_id() 是 Agent Core 内部生成的 RunId，不是业务 action_run_id。
//! 用 BoundActionCheckpointStore 绑定业务 ActionRunId，save 时用它作为外键，
//! 避免 FK 违约。take 用事务 + FOR UPDATE + status='active' 单次消费，CAS 防并发双击。

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use tracing::debug;

use agent_core::graph::{AgentCheckpoint, CheckpointError, CheckpointId, CheckpointStore};

use crate::ActionRunId;
use crate::SecretaryAgentState;

use super::store::MySqlActionStore;

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

#[derive(Debug, FromQueryResult)]
struct FullCheckpointRow {
    checkpoint_json: String,
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
