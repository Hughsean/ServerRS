use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder,
};

use super::super::entities::agent_events;
use crate::domain::agent::{
    AgentEvent, AgentEventRepoT, CHAT_APPROVAL_DECISION_EVENT, ChatApprovalAuditT,
    ChatApprovalDecisionEvent, NewAgentEvent,
};
use crate::shared::error::AppError;

pub struct AgentEventRepo {
    db: DatabaseConnection,
}

impl AgentEventRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

/// Convert a SeaORM entity [`Model`] into the domain [`AgentEvent`].
fn from_model(model: agent_events::Model) -> AgentEvent {
    AgentEvent {
        event_id: model.event_id,
        user_id: model.user_id,
        conversation_id: model.conversation_id,
        trace_id: model.trace_id,
        event_type: model.event_type,
        tool_name: model.tool_name,
        payload: model.payload.into(),
        created_at: model.created_at.and_utc(),
    }
}

#[async_trait]
impl AgentEventRepoT for AgentEventRepo {
    async fn log_event(&self, event: NewAgentEvent) -> AgentEvent {
        let now = Utc::now().naive_utc();

        let active: agent_events::ActiveModel = agent_events::ActiveModel::builder()
            .set_event_id(0_u64)
            .set_user_id(event.user_id)
            .set_conversation_id(event.conversation_id)
            .set_trace_id(None)
            .set_turn_id(None)
            .set_event_type(event.event_type)
            .set_severity("info")
            .set_tool_name(event.tool_name)
            .set_payload(event.payload)
            .set_created_at(now)
            .into();

        let saved = active
            .insert(&self.db)
            .await
            .expect("failed to insert agent_event");
        from_model(saved)
    }
}

impl AgentEventRepo {
    /// Retrieve all agent events for a given user, ordered by created_at descending.
    pub async fn find_by_user_id(&self, user_id: u64) -> Vec<AgentEvent> {
        let rows = agent_events::Entity::find()
            .filter(agent_events::Column::UserId.eq(user_id))
            .order_by(agent_events::Column::CreatedAt, Order::Desc)
            .all(&self.db)
            .await
            .expect("failed to query agent_events");

        rows.into_iter().map(from_model).collect()
    }
}

/// 审批决策审计的 MySQL 适配器，复用 `agent_events` 表。
///
/// 只写入最小字段集合；绝不记录完整 Checkpoint payload、消息历史、
/// 工具参数或认证信息。与 `AgentEventRepo` 不同，这里把写入失败作为
/// `AppError` 返回给调用方，由调用方决定仅记录日志、不影响 Resume 结果。
pub struct ChatApprovalAuditRepo {
    db: DatabaseConnection,
}

impl ChatApprovalAuditRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ChatApprovalAuditT for ChatApprovalAuditRepo {
    async fn record_decision(&self, event: ChatApprovalDecisionEvent) -> Result<(), AppError> {
        let now = Utc::now().naive_utc();
        let payload = serde_json::json!({
            "checkpoint_id": event.checkpoint_id.to_string(),
            "run_id": event.run_id.to_string(),
            "approval_id": event.approval_id.to_string(),
            "decision": event.decision.as_str(),
        });

        let active: agent_events::ActiveModel = agent_events::ActiveModel::builder()
            .set_event_id(0_u64)
            .set_user_id(event.user_id)
            .set_conversation_id(Some(event.conversation_id))
            .set_trace_id(None)
            .set_turn_id(None)
            .set_event_type(CHAT_APPROVAL_DECISION_EVENT.to_owned())
            .set_severity("info".to_owned())
            .set_tool_name(None)
            .set_payload(payload)
            .set_created_at(now)
            .into();

        // 审计不需要回读插入行，用 exec 避免 MySQL 下额外的 SELECT。
        agent_events::Entity::insert(active)
            .exec(&self.db)
            .await
            .map_err(|error| {
                tracing::error!(%error, "failed to insert tool approval decision audit");
                AppError::Infrastructure("审批决策审计暂时不可用".into())
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use agent_core::graph::{CheckpointId, RunId};
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};

    use super::*;
    use crate::domain::agent::ChatApprovalDecision;

    fn audit_event() -> ChatApprovalDecisionEvent {
        ChatApprovalDecisionEvent {
            user_id: 7,
            conversation_id: 9,
            checkpoint_id: CheckpointId::new(),
            run_id: RunId::new(),
            approval_id: uuid::Uuid::new_v4(),
            decision: ChatApprovalDecision::Approve,
        }
    }

    #[tokio::test]
    async fn record_decision_inserts_minimal_audit_row() {
        let event = audit_event();
        let checkpoint_id = event.checkpoint_id.to_string();
        let run_id = event.run_id.to_string();
        let approval_id = event.approval_id.to_string();
        let db = MockDatabase::new(DatabaseBackend::MySql)
            .append_exec_results([MockExecResult {
                last_insert_id: 42,
                rows_affected: 1,
            }])
            .into_connection();
        let log_connection = db.clone();
        let repo = ChatApprovalAuditRepo::new(db);

        repo.record_decision(event).await.expect("audit insert");

        let sql = format!("{:?}", log_connection.into_transaction_log());
        assert!(sql.contains("INSERT INTO `agent_events`"), "{sql}");
        assert!(sql.contains("tool_approval_decision"), "{sql}");
        assert!(sql.contains(&checkpoint_id), "{sql}");
        assert!(sql.contains(&run_id), "{sql}");
        assert!(sql.contains(&approval_id), "{sql}");
        assert!(sql.contains("approve"), "{sql}");
    }

    #[tokio::test]
    async fn record_decision_surfaces_insert_failures() {
        let db = MockDatabase::new(DatabaseBackend::MySql)
            .append_exec_errors([sea_orm::DbErr::Custom("insert failed".into())])
            .into_connection();
        let repo = ChatApprovalAuditRepo::new(db);

        let error = repo.record_decision(audit_event()).await.unwrap_err();

        assert!(matches!(error, AppError::Infrastructure(_)));
    }
}
