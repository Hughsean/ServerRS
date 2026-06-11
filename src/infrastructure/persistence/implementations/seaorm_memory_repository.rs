use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, Order, PaginatorTrait,
    QueryFilter, QueryOrder, Set,
};
use tracing::warn;

use super::super::entities::user_memories;

use crate::domain::memory::{MemoryRepository, NewMemory, UserMemory};
use crate::shared::error::AppError;

pub struct SeaOrmMemoryRepository {
    db: DatabaseConnection,
}

impl SeaOrmMemoryRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn map_memory(m: user_memories::Model) -> UserMemory {
    UserMemory {
        memory_id: m.memory_id,
        user_id: m.user_id,
        memory_type: m.memory_type,
        content: m.content,
        confidence: m.confidence,
        source_conversation_id: m.source_conversation_id,
        source_message_id: m.source_message_id,
        status: m.status,
        metadata: m.metadata.map(|j| j.into()),
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
    }
}

#[async_trait]
impl MemoryRepository for SeaOrmMemoryRepository {
    async fn save_memory(&self, memory: NewMemory) -> Result<UserMemory, AppError> {
        let now = Utc::now().naive_utc();
        let active = user_memories::ActiveModel {
            memory_id: sea_orm::ActiveValue::NotSet,
            user_id: Set(memory.user_id),
            memory_type: Set(memory.memory_type),
            memory_key: Set(None),
            content: Set(memory.content),
            confidence: Set(memory.confidence),
            salience: Set(0.5),
            source_conversation_id: Set(memory.source_conversation_id),
            source_message_id: Set(memory.source_message_id),
            status: Set(1),
            metadata: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            last_accessed_at: Set(None),
            access_count: Set(0),
            expires_at: Set(None),
            vector_id: Set(None),
            embedding_provider: Set(None),
            embedding_model: Set(None),
            embedding_dimension: Set(None),
            indexed_at: Set(None),
        };

        let saved = active
            .insert(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to save memory: {e}")))?;

        Ok(map_memory(saved))
    }

    async fn find_by_id(&self, memory_id: u64) -> Result<Option<UserMemory>, AppError> {
        let row = user_memories::Entity::find_by_id(memory_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to query user_memories: {e}")))?;

        Ok(row.map(map_memory))
    }

    async fn find_by_user_id(
        &self,
        user_id: u64,
        status: Option<i8>,
    ) -> Result<Vec<UserMemory>, AppError> {
        let mut query =
            user_memories::Entity::find().filter(user_memories::Column::UserId.eq(user_id));

        if let Some(s) = status {
            query = query.filter(user_memories::Column::Status.eq(s));
        }

        let rows = query
            .order_by(user_memories::Column::UpdatedAt, Order::Desc)
            .all(&self.db)
            .await
            .map_err(|e| {
                AppError::internal(format!("failed to query user_memories by user: {e}"))
            })?;

        Ok(rows.into_iter().map(map_memory).collect())
    }

    async fn search_by_user(
        &self,
        user_id: u64,
        query: &str,
        top_k: u32,
    ) -> Result<Vec<UserMemory>, AppError> {
        // LIKE-based search as fallback. The application layer should prefer
        // vector search via VectorStore when available.
        let pattern = format!("%{query}%");
        let rows = user_memories::Entity::find()
            .filter(user_memories::Column::UserId.eq(user_id))
            .filter(user_memories::Column::Content.like(&pattern))
            .filter(user_memories::Column::Status.eq(1))
            .paginate(&self.db, top_k as u64)
            .fetch_page(0)
            .await
            .map_err(|e| AppError::internal(format!("failed to search user_memories: {e}")))?;

        Ok(rows.into_iter().map(map_memory).collect())
    }

    async fn update_memory(
        &self,
        memory_id: u64,
        content: Option<String>,
        confidence: Option<f64>,
    ) -> Result<UserMemory, AppError> {
        let mut active: user_memories::ActiveModel = user_memories::Entity::find_by_id(memory_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to find memory {memory_id}: {e}")))?
            .ok_or_else(|| AppError::NotFound(format!("memory {memory_id} not found")))?
            .into();

        if let Some(c) = content {
            active.content = Set(c);
        }
        if let Some(conf) = confidence {
            active.confidence = Set(conf);
        }
        active.updated_at = Set(Utc::now().naive_utc());

        let saved = active
            .update(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to update memory {memory_id}: {e}")))?;

        Ok(map_memory(saved))
    }

    async fn disable_memory(&self, memory_id: u64) -> Result<(), AppError> {
        let mut active: user_memories::ActiveModel = user_memories::Entity::find_by_id(memory_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to find memory {memory_id}: {e}")))?
            .ok_or_else(|| AppError::NotFound(format!("memory {memory_id} not found")))?
            .into();

        active.status = Set(0);
        active.updated_at = Set(Utc::now().naive_utc());

        active.update(&self.db).await.map_err(|e| {
            AppError::internal(format!("failed to disable memory {memory_id}: {e}"))
        })?;

        Ok(())
    }

    async fn delete_memory(&self, memory_id: u64) -> Result<bool, AppError> {
        let result = user_memories::Entity::delete_by_id(memory_id)
            .exec(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to delete memory {memory_id}: {e}")))?;

        if result.rows_affected == 0 {
            warn!(memory_id, "delete_memory: no rows affected");
            return Ok(false);
        }
        Ok(true)
    }

    async fn find_memories_by_conversation(
        &self,
        conversation_id: u64,
    ) -> Result<Vec<UserMemory>, AppError> {
        let rows = user_memories::Entity::find()
            .filter(user_memories::Column::SourceConversationId.eq(conversation_id))
            .order_by(user_memories::Column::CreatedAt, Order::Asc)
            .all(&self.db)
            .await
            .map_err(|e| {
                AppError::internal(format!(
                    "failed to find memories by conversation {conversation_id}: {e}"
                ))
            })?;

        Ok(rows.into_iter().map(map_memory).collect())
    }

    async fn update_memory_index_metadata(
        &self,
        memory_id: u64,
        vector_id: String,
        embedding_provider: String,
        embedding_model: String,
        embedding_dimension: u32,
    ) -> Result<(), AppError> {
        let mut active: user_memories::ActiveModel = user_memories::Entity::find_by_id(memory_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find memory {memory_id}: {e}")))?
            .ok_or_else(|| AppError::NotFound(format!("memory {memory_id} not found")))?
            .into();
        active.vector_id = Set(Some(vector_id));
        active.embedding_provider = Set(Some(embedding_provider));
        active.embedding_model = Set(Some(embedding_model));
        active.embedding_dimension = Set(Some(embedding_dimension));
        active.indexed_at = Set(Some(Utc::now().naive_utc()));
        active.update(&self.db).await.map_err(|e| {
            AppError::internal(format!("update memory index metadata {memory_id}: {e}"))
        })?;
        Ok(())
    }

    async fn touch_memory_access(&self, memory_id: u64) -> Result<(), AppError> {
        let model = user_memories::Entity::find_by_id(memory_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find memory {memory_id}: {e}")))?
            .ok_or_else(|| AppError::NotFound(format!("memory {memory_id} not found")))?;
        let new_count = model.access_count + 1;
        let mut active: user_memories::ActiveModel = model.into();
        active.last_accessed_at = Set(Some(Utc::now().naive_utc()));
        active.access_count = Set(new_count);
        active.update(&self.db).await.map_err(|e| {
            AppError::internal(format!("touch memory access {memory_id}: {e}"))
        })?;
        Ok(())
    }

    async fn find_by_memory_key(
        &self,
        user_id: u64,
        memory_key: &str,
    ) -> Result<Option<UserMemory>, AppError> {
        let row = user_memories::Entity::find()
            .filter(user_memories::Column::UserId.eq(user_id))
            .filter(user_memories::Column::MemoryKey.eq(memory_key))
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find_by_memory_key: {e}")))?;
        Ok(row.map(map_memory))
    }

    async fn list_indexable_memories(
        &self,
        user_id: Option<u64>,
        limit: u64,
    ) -> Result<Vec<UserMemory>, AppError> {
        let mut query = user_memories::Entity::find()
            .filter(user_memories::Column::Status.eq(1))
            .filter(user_memories::Column::VectorId.is_null());
        if let Some(uid) = user_id {
            query = query.filter(user_memories::Column::UserId.eq(uid));
        }
        let rows = query
            .paginate(&self.db, limit)
            .fetch_page(0)
            .await
            .map_err(|e| AppError::internal(format!("list_indexable_memories: {e}")))?;
        Ok(rows.into_iter().map(map_memory).collect())
    }
}
