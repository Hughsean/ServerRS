use std::sync::Arc;

use tracing::warn;

use crate::application::rag::vector_index_service::VectorIndexService;
use crate::domain::memory::{ConversationSummary, NewSummary};
use crate::domain::summary::SummaryRepository;
use crate::shared::error::AppError;

/// Coordinates summary persistence and vector indexing.
pub struct SummaryService {
    summary_repo: Arc<dyn SummaryRepository>,
    vector_index: Option<Arc<VectorIndexService>>,
}

impl SummaryService {
    pub fn new(
        summary_repo: Arc<dyn SummaryRepository>,
        vector_index: Option<Arc<VectorIndexService>>,
    ) -> Self {
        Self {
            summary_repo,
            vector_index,
        }
    }

    /// Persist a summary and index it for vector search.
    /// Indexing failure does not roll back the MySQL save.
    pub async fn save_summary(&self, summary: NewSummary) -> Result<ConversationSummary, AppError> {
        let saved = self.summary_repo.save_summary(summary).await?;

        if let Some(ref vi) = self.vector_index {
            if let Err(e) = vi.index_summary(&saved).await {
                warn!(
                    summary_id = saved.summary_id,
                    error = %e,
                    "failed to index conversation summary"
                );
            }
        }

        Ok(saved)
    }

    /// Return the most recent active summary for a conversation.
    pub async fn latest_for_conversation(
        &self,
        conversation_id: u64,
    ) -> Result<Option<ConversationSummary>, AppError> {
        self.summary_repo
            .find_latest_by_conversation(conversation_id)
            .await
    }
}
