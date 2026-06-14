use super::conversation::{Conversation, NewConversation};
use super::conversation_message::{ConversationMessage, NewConversationMessage};
use crate::shared::error::AppError;
use async_trait::async_trait;

#[async_trait]
pub trait ConversationRepository: Send + Sync {
    async fn find_by_id(&self, id: u64) -> Result<Option<Conversation>, AppError>;
    async fn find_by_user_id(&self, user_id: u64) -> Result<Vec<Conversation>, AppError>;

    /// Atomic UPSERT: find existing conversation for user_id or create one.
    /// Uses MySQL `ON DUPLICATE KEY UPDATE id=LAST_INSERT_ID(id)` to ensure
    /// only one row per user_id exists. Returns the single conversation.
    async fn find_or_create_for_user(&self, user_id: u64) -> Result<Conversation, AppError>;

    /// Find the single conversation for a user (if any).
    /// Since user_id is UNIQUE, this returns Option<Conversation>.
    async fn find_single_by_user_id(&self, user_id: u64) -> Result<Option<Conversation>, AppError>;

    async fn save(&self, conv: NewConversation) -> Result<Conversation, AppError>;
    async fn update_title(&self, id: u64, title: &str) -> Result<(), AppError>;

    /// Atomically increment message_count and update last_message_at.
    /// Uses SQL `UPDATE SET message_count = message_count + ?, last_message_at = UTC_TIMESTAMP(6)`.
    async fn touch_and_incr(&self, id: u64, inc: u64) -> Result<(), AppError>;

    async fn delete_by_id(&self, id: u64) -> Result<bool, AppError>;

    async fn save_message(
        &self,
        msg: NewConversationMessage,
    ) -> Result<ConversationMessage, AppError>;
    async fn find_messages_by_conversation_id(
        &self,
        conversation_id: u64,
    ) -> Result<Vec<ConversationMessage>, AppError>;
    async fn delete_messages_by_conversation_id(
        &self,
        conversation_id: u64,
    ) -> Result<u64, AppError>;

    /// Find messages before a given ID (cursor-based pagination).
    async fn find_messages_before(
        &self,
        conversation_id: u64,
        before_id: Option<u64>,
        limit: u64,
    ) -> Result<Vec<ConversationMessage>, AppError>;

    /// Find messages since a given ID (inclusive).
    async fn find_messages_since(
        &self,
        conversation_id: u64,
        since_id: u64,
    ) -> Result<Vec<ConversationMessage>, AppError>;
}
