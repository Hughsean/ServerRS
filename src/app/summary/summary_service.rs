use std::sync::Arc;

use tracing::warn;

use crate::app::rag::vector_index_service::VectorIndexService;
use crate::domain::memory::{ConversationSummary, NewSummary, is_allowed_summary_type};
use crate::domain::summary::SummaryRepoT;
use crate::shared::error::AppError;

/// Coordinates summary persistence and vector indexing.
pub struct SummaryService {
    summary_repo: Arc<dyn SummaryRepoT>,
    vector_index: Option<Arc<VectorIndexService>>,
}

impl SummaryService {
    pub fn new(
        summary_repo: Arc<dyn SummaryRepoT>,
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
        // Validate
        if !is_allowed_summary_type(&summary.summary_type) {
            return Err(AppError::Validation(format!(
                "unsupported summary type: {}",
                summary.summary_type
            )));
        }
        if summary.content.trim().is_empty() {
            return Err(AppError::Validation(
                "summary content must not be empty".into(),
            ));
        }
        if summary.message_start_id > summary.message_end_id {
            return Err(AppError::Validation(
                "summary message range is invalid".into(),
            ));
        }

        let saved = self.summary_repo.save_summary(summary).await?;

        if let Some(ref vi) = self.vector_index {
            if let Err(e) = vi.index_summary(&saved).await {
                warn!(
                    summary_id = saved.summary_id,
                    error = %e,
                    "failed to index conversation summary"
                );
            }
            if let Some(superseded_id) = saved.supersedes_id
                && let Err(e) = vi.delete_summary_index(superseded_id).await
            {
                warn!(
                    summary_id = superseded_id,
                    error = %e,
                    "failed to delete superseded conversation summary index"
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

    /// Return the active rolling general summary used as the next refresh base.
    pub async fn latest_rolling_general(
        &self,
        conversation_id: u64,
    ) -> Result<Option<ConversationSummary>, AppError> {
        self.summary_repo
            .find_latest_rolling_general(conversation_id)
            .await
    }
}
