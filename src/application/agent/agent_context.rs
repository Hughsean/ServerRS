use std::sync::Arc;

use serde_json::Value;

use crate::application::memory::memory_service::MemoryService;
use crate::application::rag::retrieval_service::RetrievalService;
use crate::application::summary::summary_service::SummaryService;
use crate::domain::agent::{AgentContext, ToolDefinition};
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::llm::ChatMessage;
use crate::domain::user::user_profile::UserProfile;
use crate::domain::user::user_profile_repository::UserProfileRepository;

/// Builder that assembles an `AgentContext` for a single turn.
pub struct AgentContextBuilder {
    memory_service: Arc<MemoryService>,
    retrieval_service: Arc<RetrievalService>,
    summary_service: Arc<SummaryService>,
    conversation_repo: Arc<dyn ConversationRepository>,
    user_profile_repo: Arc<dyn UserProfileRepository>,
}

/// Returns the content of the most recent user message from the slice,
/// or an empty string if there are no user messages.
pub fn latest_user_query(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or("")
        .to_string()
}

impl AgentContextBuilder {
    pub fn new(
        memory_service: Arc<MemoryService>,
        retrieval_service: Arc<RetrievalService>,
        summary_service: Arc<SummaryService>,
        conversation_repo: Arc<dyn ConversationRepository>,
        user_profile_repo: Arc<dyn UserProfileRepository>,
    ) -> Self {
        Self {
            memory_service,
            retrieval_service,
            summary_service,
            conversation_repo,
            user_profile_repo,
        }
    }

    pub async fn build(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        recent_messages: Vec<ChatMessage>,
        user_profile: Option<UserProfile>,
        tools: Vec<ToolDefinition>,
        location: Option<Value>,
        max_memory_items: u32,
        max_rag_chunks: u64,
        summary_enabled: bool,
        memory_enabled: bool,
        rag_enabled: bool,
    ) -> AgentContext {
        let summary = if summary_enabled {
            if let Some(cid) = conversation_id {
                self.summary_service
                    .latest_for_conversation(cid)
                    .await
                    .unwrap_or(None)
                    .map(|s| s.content)
            } else {
                None
            }
        } else {
            None
        };

        let recall_query = latest_user_query(&recent_messages);

        let memories = if !memory_enabled || max_memory_items == 0 {
            Vec::new()
        } else {
            self.memory_service
                .recall(user_id, &recall_query, max_memory_items)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|m| {
                    format!(
                        "[{}] {} (confidence: {:.2})",
                        m.memory_type, m.content, m.confidence
                    )
                })
                .collect()
        };

        let rag_query = latest_user_query(&recent_messages);
        let rag_chunks = if !rag_enabled || rag_query.is_empty() || max_rag_chunks == 0 {
            Vec::new()
        } else {
            self.retrieval_service
                .retrieve(&rag_query, user_id, max_rag_chunks)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(chunk, _score)| chunk.content)
                .collect()
        };

        let profile: Option<Value> = match user_profile {
            Some(p) => serde_json::to_value(p).ok(),
            None => self
                .user_profile_repo
                .find_by_user_id(user_id)
                .await
                .ok()
                .flatten()
                .and_then(|p| serde_json::to_value(p).ok()),
        };

        AgentContext {
            user_id,
            conversation_id,
            recent_messages,
            summary,
            memories,
            rag_chunks,
            user_profile: profile,
            tools,
            location,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_user_query_returns_most_recent() {
        let messages = vec![
            ChatMessage {
                role: "user".into(),
                content: "第一轮问题".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: "回答".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "user".into(),
                content: "第二轮问题".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];
        assert_eq!(latest_user_query(&messages), "第二轮问题");
    }

    #[test]
    fn latest_user_query_empty_on_no_user() {
        let messages = vec![ChatMessage {
            role: "assistant".into(),
            content: "only assistant".into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        assert_eq!(latest_user_query(&messages), "");
    }

    #[test]
    fn latest_user_query_empty_on_empty_slice() {
        let messages: Vec<ChatMessage> = vec![];
        assert_eq!(latest_user_query(&messages), "");
    }
}
