use std::sync::Arc;

use crate::domain::conversation::conversation_message::ConversationMessage;
use crate::domain::conversation::conversation_repo::ConversationRepoT;
use crate::shared::error::AppError;

#[derive(Debug, Clone)]
pub struct ChatHistoryPage {
    pub messages: Vec<ConversationMessage>,
    pub next_before_id: Option<u64>,
}

pub struct ChatQueryService {
    repo: Arc<dyn ConversationRepoT>,
}

impl ChatQueryService {
    pub fn new(repo: Arc<dyn ConversationRepoT>) -> Self {
        Self { repo }
    }

    pub async fn history(
        &self,
        user_id: u64,
        before_id: Option<u64>,
        limit: u64,
    ) -> Result<ChatHistoryPage, AppError> {
        let Some(conversation) = self.repo.find_single_by_user_id(user_id).await? else {
            return Ok(ChatHistoryPage {
                messages: Vec::new(),
                next_before_id: None,
            });
        };

        let messages = self
            .repo
            .find_messages_before(conversation.id, before_id, limit)
            .await?;
        let next_before_id = (messages.len() == limit as usize)
            .then(|| messages.first().map(|message| message.id))
            .flatten();

        Ok(ChatHistoryPage {
            messages,
            next_before_id,
        })
    }
}
