use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder,
};

use super::super::entities::agent_events;
use crate::domain::agent::{AgentEvent, AgentEventRepoT, NewAgentEvent};

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
