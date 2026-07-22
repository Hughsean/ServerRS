use agent_core::AgentBusinessState;
use agent_core::graph::{
    AgentCheckpoint, AgentEffect, CheckpointError, CheckpointId, CheckpointStore, SuspendReason,
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, TransactionTrait,
};
use serde::{Serialize, de::DeserializeOwned};
use std::marker::PhantomData;
use tracing::{error, warn};

use crate::domain::agent::{
    ChatApprovalPreviewSource, ChatApprovalQueryT, CheckpointIdentity, PendingApprovalPage,
    PendingChatApproval,
};
use crate::infra::repo::entities::agent_checkpoints;
use crate::shared::error::AppError;

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

    /// 构造仅用于待审批查询的适配器实例。
    ///
    /// 查询适配器不会被当作 `CheckpointStore` 使用，TTL 传 0 使误用的
    /// `save` 立即过期，便于尽早暴露装配错误。
    pub fn for_approval_query(db: DatabaseConnection) -> Self {
        Self::new(db, 0)
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

#[async_trait]
impl<B> ChatApprovalQueryT for MySqlCheckpointStore<B>
where
    B: AgentBusinessState + CheckpointIdentity + Serialize + DeserializeOwned,
    B::Effect: AgentEffect<Update = B::Update>,
    B::SuspendData: ChatApprovalPreviewSource + Serialize + DeserializeOwned,
    <B::Effect as AgentEffect>::Receipt: Serialize + DeserializeOwned,
{
    /// 复用现有 `(user_id, status, expires_at)` 索引的非消费式查询。
    ///
    /// 每一行都经过与 `take` 相同的 payload/元数据一致性校验：数据损坏时
    /// 失败关闭并记录安全日志，绝不返回未经校验的数据。
    async fn list_pending_approvals(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        limit: u32,
    ) -> Result<PendingApprovalPage, AppError> {
        let mut query = agent_checkpoints::Entity::find()
            .filter(agent_checkpoints::Column::UserId.eq(user_id))
            .filter(agent_checkpoints::Column::Status.eq(CHECKPOINT_PENDING))
            .filter(agent_checkpoints::Column::ExpiresAt.gt(Utc::now().naive_utc()));
        if let Some(conversation_id) = conversation_id {
            query = query.filter(agent_checkpoints::Column::ConversationId.eq(conversation_id));
        }
        let rows = query
            .order_by_desc(agent_checkpoints::Column::CreatedAt)
            .order_by_desc(agent_checkpoints::Column::CheckpointId)
            .limit(u64::from(limit))
            .all(&self.db)
            .await
            .map_err(|error| approval_query_error("list pending approvals", error))?;

        let mut items = Vec::with_capacity(rows.len());
        for model in rows {
            let created_at = model.created_at.and_utc();
            let expires_at = model.expires_at.and_utc();
            let checkpoint = Self::decode(model).map_err(approval_data_error)?;
            // 非工具审批的暂停不属于待审批收件箱，跳过即可；payload 本身已通过校验。
            if let Some(approval) = checkpoint.suspend().data.approval_preview() {
                items.push(PendingChatApproval {
                    checkpoint_id: checkpoint.id(),
                    run_id: checkpoint.run_id(),
                    conversation_id: checkpoint.state().business().checkpoint_conversation_id(),
                    reason: checkpoint.suspend().reason,
                    created_at,
                    expires_at,
                    approval,
                });
            }
        }
        Ok(PendingApprovalPage { items })
    }

    async fn get_pending_approval(
        &self,
        user_id: u64,
        checkpoint_id: CheckpointId,
    ) -> Result<Option<PendingChatApproval>, AppError> {
        let Some(model) = self
            .load_pending(&self.db, checkpoint_id)
            .await
            .map_err(approval_data_error)?
        else {
            return Ok(None);
        };
        // 其他用户的 Checkpoint 与不存在一样返回 None，避免 ID 枚举。
        if model.user_id != user_id {
            return Ok(None);
        }
        let created_at = model.created_at.and_utc();
        let expires_at = model.expires_at.and_utc();
        let checkpoint = Self::decode(model).map_err(approval_data_error)?;
        Ok(checkpoint
            .suspend()
            .data
            .approval_preview()
            .map(|approval| PendingChatApproval {
                checkpoint_id: checkpoint.id(),
                run_id: checkpoint.run_id(),
                conversation_id: checkpoint.state().business().checkpoint_conversation_id(),
                reason: checkpoint.suspend().reason,
                created_at,
                expires_at,
                approval,
            }))
    }
}

fn approval_query_error(operation: &'static str, error: sea_orm::DbErr) -> AppError {
    error!(operation, %error, "pending approval query failed");
    AppError::Infrastructure("Checkpoint Store 暂时不可用".into())
}

fn approval_data_error(error: CheckpointError) -> AppError {
    match error {
        CheckpointError::NotFound { .. } => AppError::NotFound("Checkpoint 不存在或已失效".into()),
        CheckpointError::Duplicate { .. } | CheckpointError::StoreUnavailable => {
            AppError::Infrastructure("Checkpoint Store 暂时不可用".into())
        }
    }
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
    use crate::domain::agent::{ChatApprovalPreview, ChatApprovalToolCallPreview};

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

    #[derive(Clone, Serialize, serde::Deserialize)]
    struct TestSuspendData {
        approval_id: uuid::Uuid,
        prompt: String,
        tool_name: String,
    }

    impl ChatApprovalPreviewSource for TestSuspendData {
        fn approval_preview(&self) -> Option<ChatApprovalPreview> {
            Some(ChatApprovalPreview {
                approval_id: self.approval_id,
                prompt: self.prompt.clone(),
                tool_calls: vec![ChatApprovalToolCallPreview {
                    id: "call-1".into(),
                    name: self.tool_name.clone(),
                    arguments: serde_json::json!({"value": 7}),
                }],
            })
        }
    }

    impl AgentBusinessState for TestState {
        type Update = ();
        type Effect = TestEffect;
        type SuspendData = TestSuspendData;
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
            SuspendRequest::new(
                SuspendReason::Approval,
                TestSuspendData {
                    approval_id: uuid::Uuid::new_v4(),
                    prompt: "approve".into(),
                    tool_name: "controlled_tool".into(),
                },
            ),
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

    #[tokio::test]
    async fn list_pending_approvals_filters_owner_and_never_consumes() {
        let checkpoint = checkpoint();
        let model = checkpoint_model(&checkpoint);
        let db = MockDatabase::new(DatabaseBackend::MySql)
            .append_query_results([[model]])
            .into_connection();
        let log_connection = db.clone();
        let store = MySqlCheckpointStore::<TestState>::for_approval_query(db);

        let page = store
            .list_pending_approvals(7, Some(9), 20)
            .await
            .expect("list pending approvals");

        assert_eq!(page.items.len(), 1);
        let item = &page.items[0];
        assert_eq!(item.checkpoint_id, checkpoint.id());
        assert_eq!(item.run_id, checkpoint.run_id());
        assert_eq!(item.conversation_id, 9);
        assert_eq!(item.reason, SuspendReason::Approval);
        assert_eq!(item.approval.tool_calls[0].name, "controlled_tool");

        let sql = format!("{:?}", log_connection.into_transaction_log());
        // 归属、状态、过期与会话过滤都必须在 SQL 层完成
        assert!(sql.contains("`user_id` = ?"), "{sql}");
        assert!(sql.contains("`status` = ?"), "{sql}");
        assert!(sql.contains("`expires_at` > ?"), "{sql}");
        assert!(sql.contains("`conversation_id` = ?"), "{sql}");
        assert!(sql.contains("LIMIT"), "{sql}");
        // 稳定排序：created_at DESC, checkpoint_id DESC
        let order_at = sql.find("ORDER BY").expect("ORDER BY 必须存在");
        let order_clause = &sql[order_at..];
        assert!(order_clause.contains("`created_at` DESC"), "{sql}");
        assert!(order_clause.contains("`checkpoint_id` DESC"), "{sql}");
        assert!(
            order_clause.find("`created_at` DESC") < order_clause.find("`checkpoint_id` DESC"),
            "{sql}"
        );
        // 非消费式查询：绝不能出现 UPDATE/DELETE
        assert!(!sql.contains("UPDATE"), "{sql}");
        assert!(!sql.contains("DELETE"), "{sql}");
    }

    #[tokio::test]
    async fn list_pending_approvals_without_conversation_keeps_owner_scope() {
        let db = MockDatabase::new(DatabaseBackend::MySql)
            .append_query_results([Vec::<agent_checkpoints::Model>::new()])
            .into_connection();
        let log_connection = db.clone();
        let store = MySqlCheckpointStore::<TestState>::for_approval_query(db);

        let page = store.list_pending_approvals(7, None, 5).await.unwrap();

        assert!(page.items.is_empty());
        let sql = format!("{:?}", log_connection.into_transaction_log());
        assert!(sql.contains("`user_id` = ?"), "{sql}");
        assert!(!sql.contains("`conversation_id` = ?"), "{sql}");
        assert!(sql.contains("ORDER BY"), "{sql}");
        assert!(sql.contains("DESC"), "{sql}");
    }

    #[tokio::test]
    async fn list_pending_approvals_fails_closed_on_corrupted_payload() {
        let checkpoint = checkpoint();
        let mut model = checkpoint_model(&checkpoint);
        // 元数据与 payload 不一致：篡改 run_id 列
        model.run_id = RunId::new().to_string();
        let db = MockDatabase::new(DatabaseBackend::MySql)
            .append_query_results([[model]])
            .into_connection();
        let store = MySqlCheckpointStore::<TestState>::for_approval_query(db);

        let error = store.list_pending_approvals(7, None, 20).await.unwrap_err();

        assert!(matches!(error, AppError::Infrastructure(_)));
    }

    #[tokio::test]
    async fn get_pending_approval_returns_the_owner_scoped_row() {
        let checkpoint = checkpoint();
        let checkpoint_id = checkpoint.id();
        let model = checkpoint_model(&checkpoint);
        let db = MockDatabase::new(DatabaseBackend::MySql)
            .append_query_results([[model]])
            .into_connection();
        let store = MySqlCheckpointStore::<TestState>::for_approval_query(db);

        let item = store
            .get_pending_approval(7, checkpoint_id)
            .await
            .unwrap()
            .expect("pending approval exists");

        assert_eq!(item.checkpoint_id, checkpoint_id);
        assert_eq!(item.approval.prompt, "approve");
    }

    #[tokio::test]
    async fn get_pending_approval_hides_other_users_rows() {
        let checkpoint = checkpoint();
        let checkpoint_id = checkpoint.id();
        let model = checkpoint_model(&checkpoint);
        let db = MockDatabase::new(DatabaseBackend::MySql)
            .append_query_results([[model]])
            .into_connection();
        let log_connection = db.clone();
        let store = MySqlCheckpointStore::<TestState>::for_approval_query(db);

        // 行的属主是 7，查询者是 8：必须返回 None 且不得消费
        let result = store.get_pending_approval(8, checkpoint_id).await.unwrap();

        assert!(result.is_none());
        let sql = format!("{:?}", log_connection.into_transaction_log());
        assert!(!sql.contains("UPDATE"), "{sql}");
    }

    #[tokio::test]
    async fn get_pending_approval_returns_none_for_missing_rows() {
        let db = MockDatabase::new(DatabaseBackend::MySql)
            .append_query_results([Vec::<agent_checkpoints::Model>::new()])
            .into_connection();
        let store = MySqlCheckpointStore::<TestState>::for_approval_query(db);

        let result = store
            .get_pending_approval(7, CheckpointId::new())
            .await
            .unwrap();

        assert!(result.is_none());
    }
}
