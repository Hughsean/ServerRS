use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};

use crate::domain::conversation::conversation::{Conversation, NewConversation};
use crate::domain::conversation::conversation_message::{
    ConversationMessage, NewConversationMessage,
};
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::shared::error::AppError;

use super::super::entities::{conversation_messages, conversations};

pub struct SeaOrmConversationRepository {
    db: DatabaseConnection,
}

impl SeaOrmConversationRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn map_conv(m: conversations::Model) -> Conversation {
    Conversation {
        id: m.id,
        user_id: m.user_id,
        title: m.title,
        is_title_generated: m.is_title_generated != 0,
        last_message_at: m.last_message_at,
        message_count: m.message_count as i32,
        created_at: m.created_at,
    }
}

fn map_msg(m: conversation_messages::Model) -> ConversationMessage {
    ConversationMessage {
        id: m.id,
        conversation_id: m.conversation_id,
        sender_role: m.sender_role,
        sender_user_id: m.sender_user_id,
        message_type: m.message_type,
        content: serde_json::to_string(&m.content).unwrap_or_default(),
        token_count: m.token_count.map(|v| v as i32),
        created_at: m.created_at,
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl ConversationRepository for SeaOrmConversationRepository {
    async fn find_by_id(&self, id: u64) -> Result<Option<Conversation>, AppError> {
        conversations::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|o| o.map(map_conv))
    }
    async fn find_by_user_id(&self, user_id: u64) -> Result<Vec<Conversation>, AppError> {
        conversations::Entity::find()
            .filter(conversations::Column::UserId.eq(user_id))
            .order_by_desc(conversations::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(map_err)
            .map(|v| v.into_iter().map(map_conv).collect())
    }
    async fn save(&self, c: NewConversation) -> Result<Conversation, AppError> {
        let now = chrono::Utc::now();
        let am = conversations::ActiveModel {
            user_id: Set(c.user_id),
            title: Set(c.title),
            is_title_generated: Set(0_i8),
            message_count: Set(0_u32),
            created_at: Set(now),
            ..Default::default()
        };
        Ok(map_conv(am.insert(&self.db).await.map_err(map_err)?))
    }
    async fn update_title(&self, id: u64, title: &str) -> Result<(), AppError> {
        let existing = conversations::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or(AppError::NotFound("conversation not found".into()))?;
        let mut am: conversations::ActiveModel = existing.into();
        am.title = Set(Some(title.to_string()));
        am.is_title_generated = Set(1_i8);
        am.update(&self.db).await.map_err(map_err)?;
        Ok(())
    }
    async fn touch_and_incr(&self, id: u64, inc: i32) -> Result<(), AppError> {
        let existing = conversations::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or(AppError::NotFound("conversation not found".into()))?;

        let mut am: conversations::ActiveModel = existing.into();
        am.last_message_at = Set(Some(chrono::Utc::now()));
        am.message_count = Set(am
            .message_count
            .take()
            .unwrap_or(0)
            .saturating_add(inc as u32));
        am.update(&self.db).await.map_err(map_err)?;
        Ok(())
    }
    async fn delete_by_id(&self, id: u64) -> Result<bool, AppError> {
        Ok(conversations::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .map_err(map_err)?
            .rows_affected
            > 0)
    }
    async fn save_message(
        &self,
        msg: NewConversationMessage,
    ) -> Result<ConversationMessage, AppError> {
        let now = chrono::Utc::now();
        let am = conversation_messages::ActiveModel {
            conversation_id: Set(msg.conversation_id),
            sender_role: Set(msg.sender_role),
            sender_user_id: Set(msg.sender_user_id),
            message_type: Set(msg.message_type),
            content: Set(serde_json::from_str(&msg.content).unwrap_or(serde_json::Value::Null)),
            token_count: Set(msg.token_count.map(|v| v as u32)),
            created_at: Set(now),
            ..Default::default()
        };
        Ok(map_msg(am.insert(&self.db).await.map_err(map_err)?))
    }
    async fn find_messages_by_conversation_id(
        &self,
        cid: u64,
    ) -> Result<Vec<ConversationMessage>, AppError> {
        conversation_messages::Entity::find()
            .filter(conversation_messages::Column::ConversationId.eq(cid))
            .order_by_asc(conversation_messages::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(map_err)
            .map(|v| v.into_iter().map(map_msg).collect())
    }
    async fn delete_messages_by_conversation_id(&self, cid: u64) -> Result<u64, AppError> {
        Ok(conversation_messages::Entity::delete_many()
            .filter(conversation_messages::Column::ConversationId.eq(cid))
            .exec(&self.db)
            .await
            .map_err(map_err)?
            .rows_affected)
    }
}
