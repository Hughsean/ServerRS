use super::conversation::{Conversation, NewConversation};
use super::conversation_message::{ConversationMessage, NewConversationMessage};
use crate::shared::error::AppError;
use async_trait::async_trait;

#[async_trait]
pub trait ConversationRepository: Send + Sync {
    async fn find_by_id(&self, id: u64) -> Result<Option<Conversation>, AppError>;
    async fn find_by_user_id(&self, user_id: u64) -> Result<Vec<Conversation>, AppError>;

    /// 原子化 UPSERT：查找 user_id 的现有对话或创建新对话。
    /// 使用 MySQL `ON DUPLICATE KEY UPDATE id=LAST_INSERT_ID(id)` 确保
    /// only one row per user_id exists. Returns the single conversation.
    async fn find_or_create_for_user(&self, user_id: u64) -> Result<Conversation, AppError>;

    /// 查找用户的单个对话（如果有）。
    /// 由于 user_id 是 UNIQUE, this returns Option<Conversation>.
    async fn find_single_by_user_id(&self, user_id: u64) -> Result<Option<Conversation>, AppError>;

    async fn save(&self, conv: NewConversation) -> Result<Conversation, AppError>;
    async fn update_title(&self, id: u64, title: &str) -> Result<(), AppError>;

    /// 原子化增加 message_count 并更新 last_message_at。
    /// 使用 SQL `UPDATE SET message_count = message_count + ?, last_message_at = UTC_TIMESTAMP(6)`.
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

    /// 查找给定 ID 之前的消息（基于游标的分页）。
    async fn find_messages_before(
        &self,
        conversation_id: u64,
        before_id: Option<u64>,
        limit: u64,
    ) -> Result<Vec<ConversationMessage>, AppError>;

    /// 查找从给定 ID 开始的消息（包含）。
    async fn find_messages_since(
        &self,
        conversation_id: u64,
        since_id: u64,
    ) -> Result<Vec<ConversationMessage>, AppError>;

    /// 加载特定的持久化轮次，无需扫描完整对话记录。
    async fn find_messages_by_ids(
        &self,
        conversation_id: u64,
        message_ids: &[u64],
    ) -> Result<Vec<ConversationMessage>, AppError>;
}
