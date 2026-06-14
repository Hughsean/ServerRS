use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, Statement, TransactionTrait,
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

        last_message_at: m.last_message_at.map(|v| v.and_utc()),
        message_count: m.message_count as i32,
        created_at: m.created_at.and_utc(),
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
        created_at: m.created_at.and_utc(),
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

    async fn find_or_create_for_user(&self, user_id: u64) -> Result<Conversation, AppError> {
        let txn = self.db.begin().await.map_err(map_err)?;
        txn.execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO conversations (user_id, message_count, created_at, updated_at) \
             VALUES (?, 0, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)) \
             ON DUPLICATE KEY UPDATE user_id = VALUES(user_id)",
            [user_id.into()],
        ))
        .await
        .map_err(map_err)?;
        let conversation = conversations::Entity::find()
            .filter(conversations::Column::UserId.eq(user_id))
            .one(&txn)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::Internal("conversation upsert failed".into()))?;
        txn.commit().await.map_err(map_err)?;
        Ok(map_conv(conversation))
    }

    async fn find_single_by_user_id(&self, user_id: u64) -> Result<Option<Conversation>, AppError> {
        conversations::Entity::find()
            .filter(conversations::Column::UserId.eq(user_id))
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|o| o.map(map_conv))
    }

    async fn save(&self, c: NewConversation) -> Result<Conversation, AppError> {
        let now = chrono::Utc::now();
        let am = conversations::ActiveModel {
            user_id: Set(c.user_id),
            title: Set(c.title),

            message_count: Set(0),
            created_at: Set(now.naive_utc()),
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

        am.update(&self.db).await.map_err(map_err)?;
        Ok(())
    }

    /// Atomic update: message_count += inc, last_message_at = UTC_TIMESTAMP(6).
    async fn touch_and_incr(&self, id: u64, inc: u64) -> Result<(), AppError> {
        let sql = format!(
            "UPDATE conversations SET message_count = message_count + {inc}, \
             last_message_at = UTC_TIMESTAMP(6), \
             updated_at = UTC_TIMESTAMP(6) \
             WHERE id = {id}"
        );
        let stmt = Statement::from_string(DbBackend::MySql, sql);
        self.db.execute_raw(stmt).await.map_err(map_err)?;
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
            created_at: Set(now.naive_utc()),
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

    async fn find_messages_before(
        &self,
        conversation_id: u64,
        before_id: Option<u64>,
        limit: u64,
    ) -> Result<Vec<ConversationMessage>, AppError> {
        let mut query = conversation_messages::Entity::find()
            .filter(conversation_messages::Column::ConversationId.eq(conversation_id))
            .order_by_desc(conversation_messages::Column::Id);

        if let Some(before) = before_id {
            query = query.filter(conversation_messages::Column::Id.lt(before));
        }

        let rows = query
            .limit(limit as u64)
            .all(&self.db)
            .await
            .map_err(map_err)?;

        // Restore ascending order
        let mut msgs: Vec<_> = rows.into_iter().map(map_msg).collect();
        msgs.reverse();
        Ok(msgs)
    }

    async fn find_messages_since(
        &self,
        conversation_id: u64,
        since_id: u64,
    ) -> Result<Vec<ConversationMessage>, AppError> {
        conversation_messages::Entity::find()
            .filter(conversation_messages::Column::ConversationId.eq(conversation_id))
            .filter(conversation_messages::Column::Id.gte(since_id))
            .order_by_asc(conversation_messages::Column::Id)
            .all(&self.db)
            .await
            .map_err(map_err)
            .map(|v| v.into_iter().map(map_msg).collect())
    }

    async fn find_messages_by_ids(
        &self,
        conversation_id: u64,
        message_ids: &[u64],
    ) -> Result<Vec<ConversationMessage>, AppError> {
        if message_ids.is_empty() {
            return Ok(Vec::new());
        }
        conversation_messages::Entity::find()
            .filter(conversation_messages::Column::ConversationId.eq(conversation_id))
            .filter(conversation_messages::Column::Id.is_in(message_ids.iter().copied()))
            .order_by_asc(conversation_messages::Column::Id)
            .all(&self.db)
            .await
            .map_err(map_err)
            .map(|messages| messages.into_iter().map(map_msg).collect())
    }
}
