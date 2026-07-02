use async_trait::async_trait;
use sea_orm::sea_query::SimpleExpr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, Value,
};

use crate::domain::qq_bot::repository::{GroupMemory, GroupMemoryRepository};
use crate::shared::error::AppError;

use crate::infra::db::entities::qq_group_memories;

pub struct SeaOrmGroupMemoryRepository {
    db: DatabaseConnection,
}

impl SeaOrmGroupMemoryRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn model_to_domain(m: qq_group_memories::Model) -> GroupMemory {
    GroupMemory {
        group_memory_id: Some(m.group_memory_id),
        qq_group_id: m.qq_group_id,
        memory_key: m.memory_key,
        canonical_form: m.canonical_form,
        memory_type: m.memory_type,
        content: m.content,
        confidence: m.confidence,
        salience: m.salience,
        source_message_id: m.source_message_id,
        reinforce_count: m.reinforce_count,
        status: m.status,
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl GroupMemoryRepository for SeaOrmGroupMemoryRepository {
    async fn find_active_by_group(
        &self,
        qq_group_id: i64,
        limit: u32,
    ) -> Result<Vec<GroupMemory>, AppError> {
        qq_group_memories::Entity::find()
            .filter(qq_group_memories::Column::QqGroupId.eq(qq_group_id))
            .filter(qq_group_memories::Column::Status.eq(1))
            .order_by_desc(qq_group_memories::Column::Salience)
            .limit(limit as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)
            .map(|rows| rows.into_iter().map(model_to_domain).collect())
    }

    async fn upsert(&self, memory: &GroupMemory) -> Result<GroupMemory, AppError> {
        // Try to find existing by memory_key for dedup
        if let Some(ref mem_key) = memory.memory_key {
            let existing = qq_group_memories::Entity::find()
                .filter(qq_group_memories::Column::QqGroupId.eq(memory.qq_group_id))
                .filter(qq_group_memories::Column::MemoryKey.eq(mem_key))
                .one(&self.db)
                .await
                .map_err(map_db_err)?;

            if let Some(existing) = existing {
                // Update existing: increment reinforce_count, update content/confidence/salience
                let mut active: qq_group_memories::ActiveModel = existing.into();
                active.content = Set(memory.content.clone());
                active.confidence = Set(memory.confidence);
                active.salience = Set(memory.salience);
                active.reinforce_count = Set(memory.reinforce_count + 1);
                let result = active.update(&self.db).await.map_err(map_db_err)?;
                return Ok(model_to_domain(result));
            }
        }

        // Insert new
        let model = qq_group_memories::ActiveModel {
            qq_group_id: Set(memory.qq_group_id),
            memory_key: Set(memory.memory_key.clone()),
            canonical_form: Set(memory.canonical_form.clone()),
            memory_type: Set(memory.memory_type.clone()),
            content: Set(memory.content.clone()),
            confidence: Set(memory.confidence),
            salience: Set(memory.salience),
            source_message_id: Set(memory.source_message_id),
            reinforce_count: Set(memory.reinforce_count),
            status: Set(memory.status),
            ..Default::default()
        };
        let result = model.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(result))
    }

    async fn disable(&self, group_memory_id: u64) -> Result<(), AppError> {
        qq_group_memories::Entity::update_many()
            .col_expr(
                qq_group_memories::Column::Status,
                SimpleExpr::Value(Value::TinyInt(Some(0i8))),
            )
            .filter(qq_group_memories::Column::GroupMemoryId.eq(group_memory_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }
}
