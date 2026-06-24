use std::sync::Arc;

use crate::domain::qq_bot::QqBotError;
use crate::domain::qq_bot::message::{NormalizedMessage, ProcessStatus};
use crate::domain::qq_bot::repository::GroupMessageRepository;

/// Handles ingestion of OneBot group messages into the system.
///
/// Responsibilities:
/// 1. Idempotent persistence (dedup by platform_message_id)
/// 2. Set initial processing status to Pending
///
/// Note: parsing/normalization (at_bot detection, command detection, etc.) is the
/// responsibility of the caller (listener) which knows `self_qq_id`. We persist
/// whatever the caller already computed.
pub struct MessageIngestionService {
    message_repo: Arc<dyn GroupMessageRepository>,
}

impl MessageIngestionService {
    pub fn new(message_repo: Arc<dyn GroupMessageRepository>) -> Self {
        Self { message_repo }
    }

    /// Ingest an already-parsed group message into the system.
    ///
    /// Returns the persisted `NormalizedMessage` (with internal id populated).
    /// If the message already exists (dedup by platform_message_id + bot_account_id),
    /// returns the existing record without duplicating.
    pub async fn ingest(&self, msg: &NormalizedMessage) -> Result<NormalizedMessage, QqBotError> {
        // Persist (idempotent)
        let persisted = self
            .message_repo
            .insert(msg)
            .await
            .map_err(|e| QqBotError::Internal(format!("failed to persist message: {e}")))?;

        Ok(persisted)
    }

    /// Mark a message as processed (or failed).
    pub async fn mark_processed(
        &self,
        message_id: u64,
        status: ProcessStatus,
        error: Option<&str>,
    ) -> Result<(), QqBotError> {
        self.message_repo
            .update_status(message_id, status, error)
            .await
            .map_err(|e| QqBotError::Internal(format!("failed to update message status: {e}")))?;
        Ok(())
    }
}
