use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder,
    Set,
};

use super::super::entities::agent_events;
use crate::domain::agent::{AgentEvent, AgentEventRepository, NewAgentEvent};

pub struct SeaOrmAgentEventRepository {
    db: DatabaseConnection,
}

impl SeaOrmAgentEventRepository {
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
        session_id: model.session_id,
        event_type: model.event_type,
        payload: model.payload.into(),
        created_at: model.created_at.and_utc(),
    }
}

#[async_trait]
impl AgentEventRepository for SeaOrmAgentEventRepository {
    async fn log_event(&self, event: NewAgentEvent) -> AgentEvent {
        let now = Utc::now().naive_utc();

        let active = agent_events::ActiveModel {
            event_id: Set(0), // auto-increment
            user_id: Set(event.user_id),
            conversation_id: Set(event.conversation_id),
            session_id: Set(event.session_id),
            event_type: Set(event.event_type),
            payload: Set(event.payload.into()),
            created_at: Set(now),
        };

        let saved = active.insert(&self.db).await.expect("failed to insert agent_event");
        from_model(saved)
    }
}

impl SeaOrmAgentEventRepository {
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
