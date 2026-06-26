use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, Order, PaginatorTrait, QueryFilter, QueryOrder, Set, Statement, TransactionTrait,
};
use std::str::FromStr;
use tracing::warn;

use super::super::entities::{user_memories, user_memory_evidence};

use crate::domain::memory::{
    ALLOWED_MEMORY_TYPES, MemoryRepository, NewMemory, NewMemoryEvidence, UserMemory,
};
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
        reinforce_count: m.reinforce_count,
        reinforced_at: m.reinforced_at.map(|value| value.and_utc()),
        source_conversation_id: m.source_conversation_id,
        source_message_id: m.source_message_id,
        status: m.status,
        metadata: m.metadata.map(|j| j.into()),
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
    }
}

async fn insert_memory<C>(db: &C, memory: NewMemory) -> Result<UserMemory, AppError>
where
    C: ConnectionTrait,
{
    let now = Utc::now().naive_utc();
    let source_confidence =
        sea_orm::prelude::Decimal::from_str(&format!("{:.2}", memory.confidence.clamp(0.0, 1.0)))
            .unwrap_or(sea_orm::prelude::Decimal::ZERO);
    let active = user_memories::ActiveModel {
        memory_id: sea_orm::ActiveValue::NotSet,
        user_id: Set(memory.user_id),
        memory_type: Set(memory.memory_type),
        memory_key: Set(memory.memory_key),
        canonical_form: Set(memory.canonical_form),
        content: Set(memory.content),
        source_confidence: Set(source_confidence),
        confidence: Set(memory.confidence.clamp(0.0, 1.0)),
        salience: sea_orm::ActiveValue::NotSet,
        source_conversation_id: Set(memory.source_conversation_id),
        source_message_id: Set(memory.source_message_id),
        reinforced_at: Set(None),
        reinforce_count: Set(0),
        contradicted_at: Set(None),
        superseded_by: Set(None),
        status: Set(1),
        canonicalizer_version: Set(None),
        merge_decision: Set(Some(memory.merge_decision)),
        merge_reason: Set(None),
        metadata: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_accessed_at: Set(None),
        access_count: Set(0),
        vector_id: Set(None),
        embedding_provider: Set(None),
        embedding_model: Set(None),
        embedding_dimension: Set(None),
        indexed_at: Set(None),
    };

    let saved = active
        .insert(db)
        .await
        .map_err(|e| AppError::internal(format!("failed to save memory: {e}")))?;

    Ok(map_memory(saved))
}

async fn insert_evidence<C>(
    db: &C,
    memory_id: u64,
    evidence: NewMemoryEvidence,
) -> Result<(), AppError>
where
    C: ConnectionTrait,
{
    let confidence = evidence.confidence.map(|value| {
        sea_orm::prelude::Decimal::from_str(&format!("{:.3}", value.clamp(0.0, 1.0)))
            .unwrap_or(sea_orm::prelude::Decimal::ZERO)
    });
    let active = user_memory_evidence::ActiveModel {
        evidence_id: sea_orm::ActiveValue::NotSet,
        memory_id: Set(memory_id),
        source_type: Set(evidence.source_type),
        source_ref_id: Set(evidence.source_ref_id),
        message_id: Set(evidence.message_id),
        summary_id: Set(evidence.summary_id),
        source_deleted: Set(0),
        evidence_type: Set(evidence.evidence_type),
        confidence: Set(confidence),
        extractor_version: Set(evidence.extractor_version),
        created_at: Set(Utc::now().naive_utc()),
    };
    active
        .insert(db)
        .await
        .map_err(|e| AppError::internal(format!("failed to save memory evidence: {e}")))?;
    Ok(())
}

async fn bump_context_version<C>(db: &C, user_id: u64) -> Result<(), AppError>
where
    C: ConnectionTrait,
{
    let statement = Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO user_context_versions (user_id, version, updated_at) \
         VALUES (?, 2, UTC_TIMESTAMP(6)) \
         ON DUPLICATE KEY UPDATE version = version + 1, updated_at = UTC_TIMESTAMP(6)",
        [user_id.into()],
    );
    db.execute_raw(statement)
        .await
        .map_err(|e| AppError::internal(format!("bump memory context version: {e}")))?;
    Ok(())
}

#[async_trait]
impl MemoryRepository for SeaOrmMemoryRepository {
    async fn save_memory_with_evidence(
        &self,
        memory: NewMemory,
        evidence: NewMemoryEvidence,
    ) -> Result<UserMemory, AppError> {
        let txn = self
            .db
            .begin()
            .await
            .map_err(|e| AppError::internal(format!("begin memory transaction: {e}")))?;
        let saved = insert_memory(&txn, memory).await?;
        insert_evidence(&txn, saved.memory_id, evidence).await?;
        txn.commit()
            .await
            .map_err(|e| AppError::internal(format!("commit memory transaction: {e}")))?;

        Ok(saved)
    }

    async fn reinforce_memory_with_evidence(
        &self,
        memory_id: u64,
        evidence: NewMemoryEvidence,
        confidence: f64,
    ) -> Result<UserMemory, AppError> {
        let txn = self
            .db
            .begin()
            .await
            .map_err(|e| AppError::internal(format!("begin reinforcement transaction: {e}")))?;
        let model = user_memories::Entity::find_by_id(memory_id)
            .one(&txn)
            .await
            .map_err(|e| AppError::internal(format!("find memory {memory_id}: {e}")))?
            .ok_or_else(|| AppError::NotFound(format!("memory {memory_id} not found")))?;
        let mut active: user_memories::ActiveModel = model.clone().into();
        active.confidence = Set(model.confidence.max(confidence.clamp(0.0, 1.0)));
        active.reinforced_at = Set(Some(Utc::now().naive_utc()));
        active.reinforce_count = Set(model.reinforce_count.saturating_add(1));
        active.updated_at = Set(Utc::now().naive_utc());
        let updated = active
            .update(&txn)
            .await
            .map_err(|e| AppError::internal(format!("reinforce memory {memory_id}: {e}")))?;
        insert_evidence(&txn, memory_id, evidence).await?;
        txn.commit()
            .await
            .map_err(|e| AppError::internal(format!("commit reinforcement: {e}")))?;
        Ok(map_memory(updated))
    }

    async fn save_contradicting_memory_with_evidence(
        &self,
        memory: NewMemory,
        evidence: NewMemoryEvidence,
        contradicted_memory_id: u64,
    ) -> Result<UserMemory, AppError> {
        let txn = self
            .db
            .begin()
            .await
            .map_err(|e| AppError::internal(format!("begin contradiction transaction: {e}")))?;
        let contradicted = user_memories::Entity::find_by_id(contradicted_memory_id)
            .one(&txn)
            .await
            .map_err(|e| {
                AppError::internal(format!(
                    "find contradicted memory {contradicted_memory_id}: {e}"
                ))
            })?
            .ok_or_else(|| {
                AppError::NotFound(format!("memory {contradicted_memory_id} not found"))
            })?;

        let saved = insert_memory(&txn, memory).await?;
        insert_evidence(&txn, saved.memory_id, evidence).await?;

        let mut active: user_memories::ActiveModel = contradicted.into();
        active.status = Set(-1);
        active.contradicted_at = Set(Some(Utc::now().naive_utc()));
        active.superseded_by = Set(Some(saved.memory_id));
        active.updated_at = Set(Utc::now().naive_utc());
        active.update(&txn).await.map_err(|e| {
            AppError::internal(format!(
                "mark memory {contradicted_memory_id} contradicted: {e}"
            ))
        })?;
        bump_context_version(&txn, saved.user_id).await?;
        txn.commit()
            .await
            .map_err(|e| AppError::internal(format!("commit contradiction: {e}")))?;
        Ok(saved)
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
        let mut query = user_memories::Entity::find()
            .filter(user_memories::Column::UserId.eq(user_id))
            .filter(user_memories::Column::MemoryType.is_in(ALLOWED_MEMORY_TYPES.iter().copied()));

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
            .filter(user_memories::Column::MemoryType.is_in(ALLOWED_MEMORY_TYPES.iter().copied()))
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
        let txn =
            self.db.begin().await.map_err(|e| {
                AppError::internal(format!("begin disable memory transaction: {e}"))
            })?;
        let model = user_memories::Entity::find_by_id(memory_id)
            .one(&txn)
            .await
            .map_err(|e| AppError::internal(format!("failed to find memory {memory_id}: {e}")))?
            .ok_or_else(|| AppError::NotFound(format!("memory {memory_id} not found")))?;
        let user_id = model.user_id;
        let mut active: user_memories::ActiveModel = model.into();

        active.status = Set(0);
        active.updated_at = Set(Utc::now().naive_utc());

        active.update(&txn).await.map_err(|e| {
            AppError::internal(format!("failed to disable memory {memory_id}: {e}"))
        })?;
        bump_context_version(&txn, user_id).await?;
        txn.commit()
            .await
            .map_err(|e| AppError::internal(format!("commit disable memory: {e}")))?;

        Ok(())
    }

    async fn delete_memory(&self, memory_id: u64) -> Result<bool, AppError> {
        let txn = self
            .db
            .begin()
            .await
            .map_err(|e| AppError::internal(format!("begin delete memory transaction: {e}")))?;
        let Some(memory) = user_memories::Entity::find_by_id(memory_id)
            .one(&txn)
            .await
            .map_err(|e| AppError::internal(format!("find memory {memory_id}: {e}")))?
        else {
            return Ok(false);
        };
        let result = user_memories::Entity::delete_by_id(memory_id)
            .exec(&txn)
            .await
            .map_err(|e| AppError::internal(format!("failed to delete memory {memory_id}: {e}")))?;

        if result.rows_affected == 0 {
            warn!(memory_id, "delete_memory: no rows affected");
            return Ok(false);
        }
        bump_context_version(&txn, memory.user_id).await?;
        txn.commit()
            .await
            .map_err(|e| AppError::internal(format!("commit delete memory: {e}")))?;
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
        active
            .update(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("touch memory access {memory_id}: {e}")))?;
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
            .filter(user_memories::Column::MemoryType.is_in(ALLOWED_MEMORY_TYPES.iter().copied()))
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
