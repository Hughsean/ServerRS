use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, Order, PaginatorTrait, QueryFilter, QueryOrder, Set, Statement, TransactionTrait,
};

use super::super::entities::conversation_summaries;

use crate::domain::memory::{
    ALLOWED_SUMMARY_TYPES, ConversationSummary, NewSummary, ROLLING_GENERAL_SUMMARY,
};
use crate::domain::summary::SummaryRepository;
use crate::shared::error::AppError;

pub struct SeaOrmConversationSummaryRepository {
    db: DatabaseConnection,
}

impl SeaOrmConversationSummaryRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn map_summary(m: conversation_summaries::Model) -> ConversationSummary {
    ConversationSummary {
        summary_id: m.summary_id,
        conversation_id: m.conversation_id,
        user_id: m.user_id,
        summary_type: m.summary_type,
        content: m.content,
        message_start_id: m.message_start_id,
        message_end_id: m.message_end_id,
        supersedes_id: m.supersedes_id,
        word_count: m.token_count,
        status: m.status,
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
    }
}

#[async_trait]
impl SummaryRepository for SeaOrmConversationSummaryRepository {
    async fn find_latest_by_conversation(
        &self,
        conversation_id: u64,
    ) -> Result<Option<ConversationSummary>, AppError> {
        let row = conversation_summaries::Entity::find()
            .filter(conversation_summaries::Column::ConversationId.eq(conversation_id))
            .filter(conversation_summaries::Column::Status.eq(1))
            .filter(
                conversation_summaries::Column::SummaryType
                    .is_in(ALLOWED_SUMMARY_TYPES.iter().copied()),
            )
            .order_by(conversation_summaries::Column::MessageEndId, Order::Desc)
            .order_by(conversation_summaries::Column::SummaryId, Order::Desc)
            .one(&self.db)
            .await
            .map_err(|e| {
                AppError::internal(format!("failed to query conversation_summaries: {e}"))
            })?;

        Ok(row.map(map_summary))
    }

    async fn find_latest_rolling_general(
        &self,
        conversation_id: u64,
    ) -> Result<Option<ConversationSummary>, AppError> {
        let row = conversation_summaries::Entity::find()
            .filter(conversation_summaries::Column::ConversationId.eq(conversation_id))
            .filter(conversation_summaries::Column::SummaryType.eq(ROLLING_GENERAL_SUMMARY))
            .filter(conversation_summaries::Column::Status.eq(1))
            .order_by(conversation_summaries::Column::SummaryId, Order::Desc)
            .one(&self.db)
            .await
            .map_err(|e| {
                AppError::internal(format!("failed to query rolling general summary: {e}"))
            })?;

        Ok(row.map(map_summary))
    }

    async fn save_summary(&self, summary: NewSummary) -> Result<ConversationSummary, AppError> {
        let txn = self
            .db
            .begin()
            .await
            .map_err(|e| AppError::internal(format!("begin summary transaction: {e}")))?;
        let now = Utc::now().naive_utc();
        let is_rolling = summary.summary_type == ROLLING_GENERAL_SUMMARY;
        let supersedes_id = if is_rolling {
            let previous = conversation_summaries::Entity::find()
                .filter(conversation_summaries::Column::ConversationId.eq(summary.conversation_id))
                .filter(conversation_summaries::Column::SummaryType.eq(ROLLING_GENERAL_SUMMARY))
                .filter(conversation_summaries::Column::Status.eq(1))
                .order_by(conversation_summaries::Column::SummaryId, Order::Desc)
                .one(&txn)
                .await
                .map_err(|e| AppError::internal(format!("find active rolling summary: {e}")))?;

            if let Some(previous) = previous {
                let previous_id = previous.summary_id;
                let mut active: conversation_summaries::ActiveModel = previous.into();
                active.status = Set(0);
                active.updated_at = Set(now);
                active.update(&txn).await.map_err(|e| {
                    AppError::internal(format!("disable previous rolling summary: {e}"))
                })?;
                Some(previous_id)
            } else {
                None
            }
        } else {
            None
        };

        let active = conversation_summaries::ActiveModel {
            summary_id: sea_orm::ActiveValue::NotSet,
            conversation_id: Set(summary.conversation_id),
            user_id: Set(summary.user_id),
            summary_type: Set(summary.summary_type),
            content: Set(summary.content),
            message_start_id: Set(summary.message_start_id),
            message_end_id: Set(summary.message_end_id),
            token_count: Set(summary.word_count),
            status: Set(1),
            supersedes_id: Set(supersedes_id),
            created_at: Set(now),
            updated_at: Set(now),
            vector_id: Set(None),
            embedding_provider: Set(None),
            embedding_model: Set(None),
            embedding_dimension: Set(None),
            indexed_at: Set(None),
        };

        let saved = active
            .insert(&txn)
            .await
            .map_err(|e| AppError::internal(format!("failed to save conversation summary: {e}")))?;

        if is_rolling {
            let statement = Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO user_context_versions (user_id, version, updated_at) \
                 VALUES (?, 2, UTC_TIMESTAMP(6)) \
                 ON DUPLICATE KEY UPDATE version = version + 1, updated_at = UTC_TIMESTAMP(6)",
                [saved.user_id.into()],
            );
            txn.execute_raw(statement)
                .await
                .map_err(|e| AppError::internal(format!("bump summary context version: {e}")))?;
        }

        txn.commit()
            .await
            .map_err(|e| AppError::internal(format!("commit summary transaction: {e}")))?;

        Ok(map_summary(saved))
    }

    async fn find_by_id(&self, summary_id: u64) -> Result<Option<ConversationSummary>, AppError> {
        let row = conversation_summaries::Entity::find_by_id(summary_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find summary {summary_id}: {e}")))?;
        Ok(row.map(map_summary))
    }

    async fn disable_summary(&self, summary_id: u64) -> Result<(), AppError> {
        let model = conversation_summaries::Entity::find_by_id(summary_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find summary {summary_id}: {e}")))?
            .ok_or_else(|| AppError::NotFound(format!("summary {summary_id} not found")))?;
        let mut active: conversation_summaries::ActiveModel = model.into();
        active.status = Set(0);
        active
            .update(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("disable summary {summary_id}: {e}")))?;
        Ok(())
    }

    async fn list_indexable_summaries(
        &self,
        limit: u64,
    ) -> Result<Vec<ConversationSummary>, AppError> {
        let rows = conversation_summaries::Entity::find()
            .filter(conversation_summaries::Column::Status.eq(1))
            .filter(
                conversation_summaries::Column::SummaryType
                    .is_in(ALLOWED_SUMMARY_TYPES.iter().copied()),
            )
            .filter(conversation_summaries::Column::VectorId.is_null())
            .paginate(&self.db, limit)
            .fetch_page(0)
            .await
            .map_err(|e| AppError::internal(format!("list_indexable_summaries: {e}")))?;
        Ok(rows.into_iter().map(map_summary).collect())
    }

    async fn update_summary_index_metadata(
        &self,
        summary_id: u64,
        vector_id: String,
        embedding_provider: String,
        embedding_model: String,
        embedding_dimension: u32,
    ) -> Result<(), AppError> {
        let model = conversation_summaries::Entity::find_by_id(summary_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find summary {summary_id}: {e}")))?
            .ok_or_else(|| AppError::NotFound(format!("summary {summary_id} not found")))?;
        let mut active: conversation_summaries::ActiveModel = model.into();
        active.vector_id = Set(Some(vector_id));
        active.embedding_provider = Set(Some(embedding_provider));
        active.embedding_model = Set(Some(embedding_model));
        active.embedding_dimension = Set(Some(embedding_dimension));
        active.indexed_at = Set(Some(Utc::now().naive_utc()));
        active.update(&self.db).await.map_err(|e| {
            AppError::internal(format!("update summary index metadata {summary_id}: {e}"))
        })?;
        Ok(())
    }
}
