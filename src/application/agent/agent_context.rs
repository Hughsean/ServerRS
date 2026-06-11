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
        session_id: String,
        user_id: u64,
        conversation_id: Option<u64>,
        recent_messages: Vec<ChatMessage>,
        user_profile: Option<UserProfile>,
        tools: Vec<ToolDefinition>,
    ) -> AgentContext {
        let summary = if let Some(cid) = conversation_id {
            self.summary_service
                .latest_for_conversation(cid)
                .await
                .unwrap_or(None)
                .map(|s| s.content)
        } else {
            None
        };

        let query_text: String = recent_messages
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let recall_query = if query_text.is_empty() {
            "user conversation context".to_string()
        } else {
            query_text
        };

        let memories = self
            .memory_service
            .recall(user_id, &recall_query, 10)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|m| {
                format!(
                    "[{}] {} (confidence: {:.2})",
                    m.memory_type, m.content, m.confidence
                )
            })
            .collect();

        let rag_query = recent_messages
            .iter()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("")
            .to_string();
        let rag_chunks = if rag_query.is_empty() {
            Vec::new()
        } else {
            self.retrieval_service
                .retrieve(&rag_query, user_id, 5)
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
            session_id,
            conversation_id,
            recent_messages,
            summary,
            memories,
            rag_chunks,
            user_profile: profile,
            tools,
        }
    }
}
