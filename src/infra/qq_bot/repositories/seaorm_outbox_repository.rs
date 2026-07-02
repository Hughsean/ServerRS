use async_trait::async_trait;
use sea_orm::sea_query::SimpleExpr;
use sea_orm::sea_query::{Expr, ExprTrait};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, Value,
};

use crate::domain::qq_bot::repository::{OutboxEntry, OutboxRepository, OutboxStatus};
use crate::shared::error::AppError;

use crate::infra::db::entities::qq_message_outbox;

pub struct SeaOrmOutboxRepository {
    db: DatabaseConnection,
}

impl SeaOrmOutboxRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn status_to_str(s: OutboxStatus) -> &'static str {
    match s {
        OutboxStatus::Pending => "pending",
        OutboxStatus::Sending => "sending",
        OutboxStatus::Sent => "sent",
        OutboxStatus::Failed => "failed",
        OutboxStatus::Cancelled => "cancelled",
    }
}

fn status_from_str(s: &str) -> OutboxStatus {
    match s {
        "pending" => OutboxStatus::Pending,
        "sending" => OutboxStatus::Sending,
        "sent" => OutboxStatus::Sent,
        "failed" => OutboxStatus::Failed,
        "cancelled" => OutboxStatus::Cancelled,
        _ => OutboxStatus::Pending,
    }
}

fn model_to_domain(m: qq_message_outbox::Model) -> OutboxEntry {
    OutboxEntry {
        outbox_id: Some(m.outbox_id),
        bot_account_id: m.bot_account_id,
        qq_group_id: m.qq_group_id,
        qq_user_id: m.qq_user_id,
        target_type: m.target_type,
        payload: m.payload,
        related_turn_id: m.related_turn_id,
        status: status_from_str(&m.status),
        attempts: m.attempts,
        max_attempts: m.max_attempts,
        next_run_at: m.next_run_at,
        platform_message_id: m.platform_message_id,
        last_error: m.last_error,
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl OutboxRepository for SeaOrmOutboxRepository {
    async fn insert(&self, entry: &OutboxEntry) -> Result<OutboxEntry, AppError> {
        let model = qq_message_outbox::ActiveModel {
            bot_account_id: Set(entry.bot_account_id),
            qq_group_id: Set(entry.qq_group_id),
            qq_user_id: Set(entry.qq_user_id),
            target_type: Set(entry.target_type.clone()),
            payload: Set(entry.payload.clone()),
            related_turn_id: Set(entry.related_turn_id),
            status: Set(status_to_str(entry.status).to_string()),
            attempts: Set(entry.attempts),
            max_attempts: Set(entry.max_attempts),
            next_run_at: Set(entry.next_run_at),
            platform_message_id: Set(entry.platform_message_id.clone()),
            last_error: Set(entry.last_error.clone()),
            ..Default::default()
        };
        let result = model.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(result))
    }

    async fn fetch_due(&self, limit: u32) -> Result<Vec<OutboxEntry>, AppError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        qq_message_outbox::Entity::find()
            .filter(qq_message_outbox::Column::Status.eq("pending"))
            .filter(qq_message_outbox::Column::NextRunAt.lte(now))
            .order_by_asc(qq_message_outbox::Column::NextRunAt)
            .limit(limit as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)
            .map(|rows| rows.into_iter().map(model_to_domain).collect())
    }

    async fn mark_sent(&self, outbox_id: u64, platform_message_id: &str) -> Result<(), AppError> {
        qq_message_outbox::Entity::update_many()
            .col_expr(
                qq_message_outbox::Column::Status,
                SimpleExpr::Value(Value::String(Some("sent".to_string()))),
            )
            .col_expr(
                qq_message_outbox::Column::PlatformMessageId,
                SimpleExpr::Value(Value::String(Some(platform_message_id.to_string()))),
            )
            .filter(qq_message_outbox::Column::OutboxId.eq(outbox_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn mark_retry(
        &self,
        outbox_id: u64,
        error: &str,
        next_run_at: i64,
    ) -> Result<(), AppError> {
        qq_message_outbox::Entity::update_many()
            .col_expr(
                qq_message_outbox::Column::Attempts,
                Expr::col(qq_message_outbox::Column::Attempts).add(1).into(),
            )
            .col_expr(
                qq_message_outbox::Column::NextRunAt,
                SimpleExpr::Value(Value::BigInt(Some(next_run_at))),
            )
            .col_expr(
                qq_message_outbox::Column::LastError,
                SimpleExpr::Value(Value::String(Some(error.to_string()))),
            )
            .filter(qq_message_outbox::Column::OutboxId.eq(outbox_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn mark_failed(&self, outbox_id: u64, error: &str) -> Result<(), AppError> {
        qq_message_outbox::Entity::update_many()
            .col_expr(
                qq_message_outbox::Column::Attempts,
                Expr::col(qq_message_outbox::Column::Attempts).add(1).into(),
            )
            .col_expr(
                qq_message_outbox::Column::Status,
                SimpleExpr::Value(Value::String(Some("failed".to_string()))),
            )
            .col_expr(
                qq_message_outbox::Column::LastError,
                SimpleExpr::Value(Value::String(Some(error.to_string()))),
            )
            .filter(qq_message_outbox::Column::OutboxId.eq(outbox_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn mark_cancelled(&self, outbox_id: u64) -> Result<(), AppError> {
        qq_message_outbox::Entity::update_many()
            .col_expr(
                qq_message_outbox::Column::Status,
                SimpleExpr::Value(Value::String(Some("cancelled".to_string()))),
            )
            .filter(qq_message_outbox::Column::OutboxId.eq(outbox_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }
}
