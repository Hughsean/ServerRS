use std::sync::Arc;

use crate::domain::qq_bot::QqBotError;
use crate::domain::qq_bot::message::{MessageDirection, NormalizedMessage, ProcessStatus};
use crate::domain::qq_bot::repository::GroupMessageRepository;
use crate::infra::qq_bot::napcat::message_parser::{normalize_text, parse_message_segments};

/// Handles ingestion of raw OneBot group messages into the system.
///
/// Responsibilities:
/// 1. Parse raw CQ-code messages into structured `NormalizedMessage`
/// 2. Idempotent persistence (dedup by platform_message_id)
/// 3. Set initial processing status to Pending
pub struct MessageIngestionService {
    message_repo: Arc<dyn GroupMessageRepository>,
}

impl MessageIngestionService {
    pub fn new(message_repo: Arc<dyn GroupMessageRepository>) -> Self {
        Self { message_repo }
    }

    /// Ingest a raw group message from OneBot into the system.
    ///
    /// Returns the persisted `NormalizedMessage` (with internal id populated).
    /// If the message already exists (dedup by platform_message_id + bot_account_id),
    /// returns the existing record without duplicating.
    pub async fn ingest(
        &self,
        bot_account_id: u64,
        group_id: i64,
        user_id: i64,
        platform_message_id: &str,
        raw_text: &str,
        sent_at: i64,
    ) -> Result<NormalizedMessage, QqBotError> {
        // Parse and normalize
        let (normalized_text, at_bot, command_name) = normalize_text(raw_text, 0); // self_qq_id checked by listener
        let segments = parse_message_segments(raw_text);

        let msg = NormalizedMessage {
            id: None,
            bot_account_id,
            qq_group_id: group_id,
            qq_user_id: Some(user_id),
            platform_message_id: platform_message_id.to_string(),
            direction: MessageDirection::Inbound,
            raw_text: raw_text.to_string(),
            normalized_text,
            segments,
            at_bot,
            command_name,
            sent_at,
        };

        // Persist (idempotent)
        let persisted = self
            .message_repo
            .insert(&msg)
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
