use async_trait::async_trait;

use crate::domain::memory::{ConversationSummary, NewSummary};
use crate::shared::error::AppError;

/// Repository for conversation summaries.
///
/// Lives in the domain layer so infrastructure implementations can depend
/// on it without coupling to the application layer.
#[async_trait]
pub trait SummaryRepository: Send + Sync {
    /// Load the most recent active summary for a conversation.
    async fn find_latest_by_conversation(
        &self,
        conversation_id: u64,
    ) -> Result<Option<ConversationSummary>, AppError>;

    /// Persist a new summary and return the saved record.
    async fn save_summary(&self, summary: NewSummary) -> Result<ConversationSummary, AppError>;

    /// Look up a summary by primary key.
    async fn find_by_id(
        &self,
        summary_id: u64,
    ) -> Result<Option<ConversationSummary>, AppError>;

    /// Soft-disable a summary (status = 0).
    async fn disable_summary(&self, summary_id: u64) -> Result<(), AppError>;

    /// List summaries eligible for vector indexing (status = 1, unindexed).
    async fn list_indexable_summaries(
        &self,
        limit: u64,
    ) -> Result<Vec<ConversationSummary>, AppError>;

    /// Update vector-index metadata on a summary row.
    async fn update_summary_index_metadata(
        &self,
        summary_id: u64,
        vector_id: String,
        embedding_provider: String,
        embedding_model: String,
        embedding_dimension: u32,
    ) -> Result<(), AppError>;
}
