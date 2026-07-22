use agent_core::AgentBusinessState;
use agent_core::graph::{
    AgentCheckpoint, AgentEffect, CheckpointError, CheckpointId, CheckpointStore, SuspendReason,
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    TransactionTrait,
};
use serde::{Serialize, de::DeserializeOwned};
use std::marker::PhantomData;
use tracing::{error, warn};

use crate::domain::agent::CheckpointIdentity;
use crate::infra::repo::entities::agent_checkpoints;

const CHECKPOINT_PENDING: &str = "pending";
const CHECKPOINT_CONSUMED: &str = "consumed";

/// 基于 MySQL 的 HTTP Chat Checkpoint Store。
///
/// `take` 使用带状态条件的单行更新，因此多个服务进程竞争恢复同一个
/// Checkpoint 时，只有一个进程能够成功消费。
pub struct MySqlCheckpointStore<B> {
    db: DatabaseConnection,
    ttl: Duration,
    state: PhantomData<fn() -> B>,
}

impl<B> MySqlCheckpointStore<B>
where
    B: AgentBusinessState + CheckpointIdentity + Serialize + DeserializeOwned,
    B::Effect: AgentEffect<Update = B::Update>,
    B::SuspendData: Serialize + DeserializeOwned,
    <B::Effect as AgentEffect>::Receipt: Serialize + DeserializeOwned,
{
    pub fn new(db: DatabaseConnection, ttl_secs: u64) -> Self {
        let ttl_seconds = i64::try_from(ttl_secs).unwrap_or(i64::MAX);
        Self {
            db,
            ttl: Duration::seconds(ttl_seconds),
            state: PhantomData,
        }
    }

    async fn load_pending<C>(
        &self,
        db: &C,
        checkpoint_id: CheckpointId,
    ) -> Result<Option<agent_checkpoints::Model>, CheckpointError>
    where
        C: ConnectionTrait,
    {
        agent_checkpoints::Entity::find_by_id(checkpoint_id.to_string())
            .filter(agent_checkpoints::Column::Status.eq(CHECKPOINT_PENDING))
            .filter(agent_checkpoints::Column::ExpiresAt.gt(Utc::now().naive_utc()))
            .one(db)
            .await
            .map_err(|error| store_error("load checkpoint", error))
    }

    async fn purge_expired(&self) {
        if let Err(error) = agent_checkpoints::Entity::delete_many()
            .filter(agent_checkpoints::Column::ExpiresAt.lte(Utc::now().naive_utc()))
            .exec(&self.db)
            .await
        {
            warn!(%error, "failed to purge expired agent checkpoints");
        }
    }

    fn decode(model: agent_checkpoints::Model) -> Result<AgentCheckpoint<B>, CheckpointError> {
        let checkpoint: AgentCheckpoint<B> = serde_json::from_value(model.payload.clone())
            .map_err(|error| {
                error!(
                    checkpoint_id = %model.checkpoint_id,
                    %error,
                    "failed to deserialize agent checkpoint"
                );
                CheckpointError::StoreUnavailable
            })?;

        let metadata_matches = checkpoint.id().to_string() == model.checkpoint_id
            && checkpoint.run_id().to_string() == model.run_id
            && checkpoint.graph_id().to_string() == model.graph_id
            && checkpoint.graph_version().get() == model.graph_version
            && checkpoint.state_schema_version().get() == model.state_schema_version
            && checkpoint.state().business().checkpoint_user_id() == model.user_id
            && checkpoint.state().business().checkpoint_conversation_id() == model.conversation_id
            && checkpoint.position().next_node().to_string() == model.next_node
            && checkpoint.position().completed_step().get() == model.completed_step
            && suspend_reason_name(checkpoint.suspend().reason) == model.suspend_reason;
        if !metadata_matches {
            error!(
                checkpoint_id = %model.checkpoint_id,
                "agent checkpoint metadata does not match payload"
            );
            return Err(CheckpointError::StoreUnavailable);
        }

        Ok(checkpoint)
    }
}

#[async_trait]
impl<B> CheckpointStore<B> for MySqlCheckpointStore<B>
where
    B: AgentBusinessState + CheckpointIdentity + Serialize + DeserializeOwned,
    B::Effect: AgentEffect<Update = B::Update>,
    B::SuspendData: Serialize + DeserializeOwned,
    <B::Effect as AgentEffect>::Receipt: Serialize + DeserializeOwned,
{
    async fn save(&self, checkpoint: AgentCheckpoint<B>) -> Result<(), CheckpointError> {
        self.purge_expired().await;
        let checkpoint_id = checkpoint.id();
        if agent_checkpoints::Entity::find_by_id(checkpoint_id.to_string())
            .one(&self.db)
            .await
            .map_err(|error| store_error("check checkpoint uniqueness", error))?
            .is_some()
        {
            return Err(CheckpointError::Duplicate { checkpoint_id });
        }

        let payload = serde_json::to_value(&checkpoint).map_err(|error| {
            error!(%checkpoint_id, %error, "failed to serialize agent checkpoint");
            CheckpointError::StoreUnavailable
        })?;
        let now = Utc::now().naive_utc();
        let expires_at = now.checked_add_signed(self.ttl).ok_or_else(|| {
            error!(%checkpoint_id, "agent checkpoint expiry overflow");
            CheckpointError::StoreUnavailable
        })?;
        let turn = checkpoint.state().business();
        let model: agent_checkpoints::ActiveModel = agent_checkpoints::ActiveModel::builder()
            .set_checkpoint_id(checkpoint_id.to_string())
            .set_run_id(checkpoint.run_id().to_string())
            .set_graph_id(checkpoint.graph_id().to_string())
            .set_graph_version(checkpoint.graph_version().get())
            .set_state_schema_version(checkpoint.state_schema_version().get())
            .set_user_id(turn.checkpoint_user_id())
            .set_conversation_id(turn.checkpoint_conversation_id())
            .set_next_node(checkpoint.position().next_node().to_string())
            .set_completed_step(checkpoint.position().completed_step().get())
            .set_suspend_reason(suspend_reason_name(checkpoint.suspend().reason))
            .set_payload(payload)
            .set_status(CHECKPOINT_PENDING)
            .set_expires_at(expires_at)
            .set_consumed_at(None)
            .set_created_at(now)
            .set_updated_at(now)
            .into();
        model
            .insert(&self.db)
            .await
            .map_err(|error| store_error("save checkpoint", error))?;
        Ok(())
    }

    async fn load(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<AgentCheckpoint<B>, CheckpointError> {
        let model = self
            .load_pending(&self.db, checkpoint_id)
            .await?
            .ok_or(CheckpointError::NotFound { checkpoint_id })?;
        Self::decode(model)
    }

    async fn take(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<AgentCheckpoint<B>, CheckpointError> {
        let transaction = self
            .db
            .begin()
            .await
            .map_err(|error| store_error("begin checkpoint consume", error))?;
        let model = self
            .load_pending(&transaction, checkpoint_id)
            .await?
            .ok_or(CheckpointError::NotFound { checkpoint_id })?;
        let checkpoint = Self::decode(model)?;
        let now = Utc::now().naive_utc();
        let update = agent_checkpoints::Entity::update_many()
            .col_expr(
                agent_checkpoints::Column::Status,
                Expr::value(CHECKPOINT_CONSUMED),
            )
            .col_expr(
                agent_checkpoints::Column::ConsumedAt,
                Expr::value(Some(now)),
            )
            .col_expr(agent_checkpoints::Column::UpdatedAt, Expr::value(now))
            .filter(agent_checkpoints::Column::CheckpointId.eq(checkpoint_id.to_string()))
            .filter(agent_checkpoints::Column::Status.eq(CHECKPOINT_PENDING))
            .filter(agent_checkpoints::Column::ExpiresAt.gt(now))
            .exec(&transaction)
            .await
            .map_err(|error| store_error("consume checkpoint", error))?;
        if update.rows_affected != 1 {
            transaction
                .rollback()
                .await
                .map_err(|error| store_error("rollback lost checkpoint claim", error))?;
            return Err(CheckpointError::NotFound { checkpoint_id });
        }
        transaction
            .commit()
            .await
            .map_err(|error| store_error("commit checkpoint consume", error))?;
        Ok(checkpoint)
    }
}

fn suspend_reason_name(reason: SuspendReason) -> &'static str {
    match reason {
        SuspendReason::ExternalInput => "external_input",
        SuspendReason::Approval => "approval",
        SuspendReason::ExternalEvent => "external_event",
        SuspendReason::Business => "business",
    }
}

fn store_error(operation: &'static str, error: sea_orm::DbErr) -> CheckpointError {
    error!(operation, %error, "agent checkpoint store operation failed");
    CheckpointError::StoreUnavailable
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::time::Duration as StdDuration;

    use agent_core::graph::{
        GraphId, GraphVersion, NodeId, RunBudget, RunId, RunPosition, RunStep, RunTrace,
        SuspendRequest, UsageSnapshot,
    };
    use agent_core::{AgentState, AgentStateError, AgentUpdate};
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};

    use super::*;

    #[derive(Clone, Serialize, serde::Deserialize)]
    struct TestState {
        user_id: u64,
        conversation_id: u64,
    }

    enum TestEffect {}

    #[derive(Clone, Serialize, serde::Deserialize)]
    struct TestReceipt;

    impl AgentEffect for TestEffect {
        type Update = ();
        type Receipt = TestReceipt;

        fn receipt_updates(_receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>> {
            Vec::new()
        }
    }

    impl AgentBusinessState for TestState {
        type Update = ();
        type Effect = TestEffect;
        type SuspendData = String;
        type ResumeInput = ();

        fn resume_updates(_input: Self::ResumeInput) -> Vec<AgentUpdate<Self::Update>> {
            Vec::new()
        }

        fn apply_update(&mut self, _update: Self::Update) -> Result<(), AgentStateError> {
            Ok(())
        }
    }

    impl CheckpointIdentity for TestState {
        fn checkpoint_user_id(&self) -> u64 {
            self.user_id
        }

        fn checkpoint_conversation_id(&self) -> u64 {
            self.conversation_id
        }
    }

    fn checkpoint() -> AgentCheckpoint<TestState> {
        AgentCheckpoint::new(
            CheckpointId::new(),
            GraphId::try_from("http_chat_agent").unwrap(),
            GraphVersion::try_from(3).unwrap(),
            TestState::state_schema_version(),
            RunId::new(),
            RunPosition::new(
                RunStep::try_from(1).unwrap(),
                NodeId::try_from("reasoning.tools").unwrap(),
            ),
            AgentState::new(TestState {
                user_id: 7,
                conversation_id: 9,
            }),
            RunBudget::new(NonZeroU32::new(20).unwrap(), StdDuration::from_secs(60)),
            UsageSnapshot {
                steps: 1,
                ..UsageSnapshot::default()
            },
            vec![NodeId::try_from("reasoning.approval_gate").unwrap()],
            vec![],
            SuspendRequest::new(SuspendReason::Approval, "approve".into()),
            RunTrace::default(),
        )
    }

    fn checkpoint_model(checkpoint: &AgentCheckpoint<TestState>) -> agent_checkpoints::Model {
        let now = Utc::now().naive_utc();
        agent_checkpoints::Model {
            checkpoint_id: checkpoint.id().to_string(),
            run_id: checkpoint.run_id().to_string(),
            graph_id: checkpoint.graph_id().to_string(),
            graph_version: checkpoint.graph_version().get(),
            state_schema_version: checkpoint.state_schema_version().get(),
            user_id: 7,
            conversation_id: 9,
            next_node: checkpoint.position().next_node().to_string(),
            completed_step: checkpoint.position().completed_step().get(),
            suspend_reason: "approval".into(),
            payload: serde_json::to_value(checkpoint).unwrap(),
            status: CHECKPOINT_PENDING.into(),
            expires_at: now + Duration::hours(1),
            consumed_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn take_round_trips_payload_and_claims_only_pending_unexpired_row() {
        let checkpoint = checkpoint();
        let checkpoint_id = checkpoint.id();
        let model = checkpoint_model(&checkpoint);
        let db = MockDatabase::new(DatabaseBackend::MySql)
            .append_query_results([[model]])
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();
        let log_connection = db.clone();
        let store = MySqlCheckpointStore::<TestState>::new(db, 3_600);

        let taken = store.take(checkpoint_id).await.unwrap();

        assert_eq!(taken.id(), checkpoint_id);
        assert_eq!(taken.run_id(), checkpoint.run_id());
        let sql = format!("{:?}", log_connection.into_transaction_log());
        assert!(sql.contains("BEGIN"), "{sql}");
        assert!(sql.contains("COMMIT"), "{sql}");
        assert!(
            sql.contains("UPDATE `agent_checkpoints` SET `status` = ?"),
            "{sql}"
        );
        assert!(sql.matches("`status` = ?").count() >= 2, "{sql}");
        assert!(sql.contains("`expires_at` > ?"), "{sql}");
        assert!(sql.contains("String(Some(\"pending\"))"), "{sql}");
        assert!(sql.contains("String(Some(\"consumed\"))"), "{sql}");
        assert!(sql.contains(&checkpoint_id.to_string()), "{sql}");
    }

    #[tokio::test]
    async fn losing_atomic_claim_returns_not_found_and_rolls_back() {
        let checkpoint = checkpoint();
        let checkpoint_id = checkpoint.id();
        let db = MockDatabase::new(DatabaseBackend::MySql)
            .append_query_results([[checkpoint_model(&checkpoint)]])
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            }])
            .into_connection();
        let log_connection = db.clone();
        let store = MySqlCheckpointStore::<TestState>::new(db, 3_600);

        let error = store.take(checkpoint_id).await.unwrap_err();

        assert_eq!(error, CheckpointError::NotFound { checkpoint_id });
        let sql = format!("{:?}", log_connection.into_transaction_log());
        assert!(sql.contains("ROLLBACK"), "{sql}");
        assert!(!sql.contains("COMMIT"), "{sql}");
    }
}
