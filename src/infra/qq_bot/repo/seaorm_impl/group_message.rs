use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};

use crate::domain::qq_bot::message::{MessageDirection, NormalizedMessage, ProcessStatus};
use crate::domain::qq_bot::repository::GroupMessageRepoT;
use crate::shared::error::AppError;

use crate::infra::repo::entities::qq_group_messages;

pub struct GroupMessageRepo {
    db: DatabaseConnection,
}

impl GroupMessageRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn direction_to_str(d: MessageDirection) -> &'static str {
    match d {
        MessageDirection::Inbound => "inbound",
        MessageDirection::Outbound => "outbound",
    }
}

fn direction_from_str(s: &str) -> MessageDirection {
    match s {
        "inbound" => MessageDirection::Inbound,
        "outbound" => MessageDirection::Outbound,
        _ => MessageDirection::Inbound,
    }
}

fn model_to_domain(m: qq_group_messages::Model) -> NormalizedMessage {
    NormalizedMessage {
        id: Some(m.id),
        bot_account_id: m.bot_account_id,
        qq_group_id: m.qq_group_id,
        qq_user_id: m.qq_user_id,
        platform_message_id: m.platform_message_id,
        direction: direction_from_str(&m.direction),
        raw_text: m.raw_text,
        normalized_text: m.normalized_text,
        segments: serde_json::from_value(m.segments).unwrap_or_default(),
        at_bot: m.at_bot != 0,
        command_name: m.command_name,
        sent_at: m.sent_at,
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl GroupMessageRepoT for GroupMessageRepo {
    async fn insert(&self, msg: &NormalizedMessage) -> Result<NormalizedMessage, AppError> {
        // Idempotency: check if message already exists by unique key
        let existing = qq_group_messages::Entity::find()
            .filter(qq_group_messages::Column::BotAccountId.eq(msg.bot_account_id))
            .filter(qq_group_messages::Column::PlatformMessageId.eq(&msg.platform_message_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        if let Some(existing) = existing {
            return Ok(model_to_domain(existing));
        }

        let segments_json =
            serde_json::to_value(&msg.segments).unwrap_or_else(|_| serde_json::Value::Null);

        let model: qq_group_messages::ActiveModel = qq_group_messages::ActiveModel::builder()
            .set_bot_account_id(msg.bot_account_id)
            .set_qq_group_id(msg.qq_group_id)
            .set_qq_user_id(msg.qq_user_id)
            .set_platform_message_id(msg.platform_message_id.clone())
            .set_direction(direction_to_str(msg.direction))
            .set_raw_text(msg.raw_text.clone())
            .set_normalized_text(msg.normalized_text.clone())
            .set_segments(segments_json)
            .set_at_bot(if msg.at_bot { 1_i8 } else { 0_i8 })
            .set_command_name(msg.command_name.clone())
            .set_sent_at(msg.sent_at)
            .set_status("pending")
            .into();
        let result = model.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(result))
    }

    async fn find_by_platform_id(
        &self,
        bot_account_id: u64,
        platform_message_id: &str,
    ) -> Result<Option<NormalizedMessage>, AppError> {
        qq_group_messages::Entity::find()
            .filter(qq_group_messages::Column::BotAccountId.eq(bot_account_id))
            .filter(qq_group_messages::Column::PlatformMessageId.eq(platform_message_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn recent_by_group(
        &self,
        qq_group_id: i64,
        limit: u32,
    ) -> Result<Vec<NormalizedMessage>, AppError> {
        qq_group_messages::Entity::find()
            .filter(qq_group_messages::Column::QqGroupId.eq(qq_group_id))
            .order_by_asc(qq_group_messages::Column::SentAt)
            .limit(limit as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)
            .map(|rows| rows.into_iter().map(model_to_domain).collect())
    }

    async fn update_status(
        &self,
        id: u64,
        _status: ProcessStatus,
        error: Option<&str>,
    ) -> Result<(), AppError> {
        let status_str = match _status {
            ProcessStatus::Pending => "pending",
            ProcessStatus::Ignored => "ignored",
            ProcessStatus::Processed => "processed",
            ProcessStatus::Failed => "failed",
        };

        let existing = qq_group_messages::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| AppError::NotFound(format!("message {id} not found")))?;

        let mut active: qq_group_messages::ActiveModel = existing.into();
        active.status = Set(status_str.to_string());
        // error field is not in the entity, so we ignore it
        let _ = error;
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
