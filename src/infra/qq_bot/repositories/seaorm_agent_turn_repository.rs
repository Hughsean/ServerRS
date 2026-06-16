use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};

use crate::domain::qq_bot::repository::AgentTurnRepository;
use crate::domain::qq_bot::turn::{AgentTurn, TriggerType, TurnStatus};
use crate::shared::error::AppError;

use super::super::super::persistence::entities::qq_agent_turns;

pub struct SeaOrmAgentTurnRepository {
    db: DatabaseConnection,
}

impl SeaOrmAgentTurnRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn trigger_type_to_str(t: TriggerType) -> &'static str {
    match t {
        TriggerType::Mention => "mention",
        TriggerType::Keyword => "keyword",
        TriggerType::Command => "command",
        TriggerType::Always => "always",
        TriggerType::Manual => "manual",
    }
}

fn trigger_type_from_str(s: &str) -> TriggerType {
    match s {
        "mention" => TriggerType::Mention,
        "keyword" => TriggerType::Keyword,
        "command" => TriggerType::Command,
        "always" => TriggerType::Always,
        "manual" => TriggerType::Manual,
        _ => TriggerType::Mention,
    }
}

fn turn_status_to_str(s: TurnStatus) -> &'static str {
    match s {
        TurnStatus::Created => "created",
        TurnStatus::Responded => "responded",
        TurnStatus::Failed => "failed",
        TurnStatus::Cancelled => "cancelled",
    }
}

fn turn_status_from_str(s: &str) -> TurnStatus {
    match s {
        "created" => TurnStatus::Created,
        "responded" => TurnStatus::Responded,
        "failed" => TurnStatus::Failed,
        "cancelled" => TurnStatus::Cancelled,
        _ => TurnStatus::Created,
    }
}

fn model_to_domain(m: qq_agent_turns::Model) -> AgentTurn {
    AgentTurn {
        turn_id: Some(m.turn_id),
        bot_account_id: m.bot_account_id,
        qq_group_id: m.qq_group_id,
        trigger_message_id: m.trigger_message_id,
        response_message_id: m.response_message_id,
        trigger_type: trigger_type_from_str(&m.trigger_type),
        qq_user_id: m.qq_user_id,
        internal_user_id: m.internal_user_id,
        prompt_version: m.prompt_version,
        model_name: m.model_name,
        reasoning_enabled: m.reasoning_enabled.map(|v| v != 0),
        input_token_count: m.input_token_count,
        output_token_count: m.output_token_count,
        latency_ms: m.latency_ms,
        status: turn_status_from_str(&m.status),
        error_message: m.error_message,
        trace_id: m.trace_id,
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl AgentTurnRepository for SeaOrmAgentTurnRepository {
    async fn insert(&self, turn: &AgentTurn) -> Result<AgentTurn, AppError> {
        let model = qq_agent_turns::ActiveModel {
            bot_account_id: Set(turn.bot_account_id),
            qq_group_id: Set(turn.qq_group_id),
            trigger_message_id: Set(turn.trigger_message_id),
            response_message_id: Set(turn.response_message_id),
            trigger_type: Set(trigger_type_to_str(turn.trigger_type).to_string()),
            qq_user_id: Set(turn.qq_user_id),
            internal_user_id: Set(turn.internal_user_id),
            prompt_version: Set(turn.prompt_version.clone()),
            model_name: Set(turn.model_name.clone()),
            reasoning_enabled: Set(turn.reasoning_enabled.map(|v| if v { 1i8 } else { 0i8 })),
            input_token_count: Set(turn.input_token_count),
            output_token_count: Set(turn.output_token_count),
            latency_ms: Set(turn.latency_ms),
            status: Set(turn_status_to_str(turn.status).to_string()),
            error_message: Set(turn.error_message.clone()),
            trace_id: Set(turn.trace_id.clone()),
            created_at: Set(turn.created_at.naive_utc()),
            updated_at: Set(turn.updated_at.naive_utc()),
            ..Default::default()
        };
        let result = model.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(result))
    }

    async fn update_response(
        &self,
        turn_id: u64,
        response_message_id: u64,
        status: TurnStatus,
    ) -> Result<(), AppError> {
        use sea_orm::sea_query::SimpleExpr;
        qq_agent_turns::Entity::update_many()
            .col_expr(
                qq_agent_turns::Column::ResponseMessageId,
                SimpleExpr::Value(sea_orm::Value::BigUnsigned(Some(response_message_id))),
            )
            .col_expr(
                qq_agent_turns::Column::Status,
                SimpleExpr::Value(sea_orm::Value::String(Some(
                    turn_status_to_str(status).to_string(),
                ))),
            )
            .filter(qq_agent_turns::Column::TurnId.eq(turn_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn update_status(
        &self,
        turn_id: u64,
        status: TurnStatus,
        error: Option<&str>,
    ) -> Result<(), AppError> {
        // Fetch existing, modify, persist
        let existing = qq_agent_turns::Entity::find_by_id(turn_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| AppError::NotFound(format!("agent turn {turn_id} not found")))?;

        let mut active: qq_agent_turns::ActiveModel = existing.into();
        active.status = Set(turn_status_to_str(status).to_string());
        if let Some(msg) = error {
            active.error_message = Set(Some(msg.to_string()));
        }
        active.updated_at = Set(chrono::Utc::now().naive_utc());
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn find_by_trace_id(&self, trace_id: &str) -> Result<Option<AgentTurn>, AppError> {
        qq_agent_turns::Entity::find()
            .filter(qq_agent_turns::Column::TraceId.eq(trace_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn recent_by_group(
        &self,
        qq_group_id: i64,
        limit: u32,
    ) -> Result<Vec<AgentTurn>, AppError> {
        qq_agent_turns::Entity::find()
            .filter(qq_agent_turns::Column::QqGroupId.eq(qq_group_id))
            .order_by_desc(qq_agent_turns::Column::CreatedAt)
            .limit(limit as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)
            .map(|rows| rows.into_iter().map(model_to_domain).collect())
    }
}
