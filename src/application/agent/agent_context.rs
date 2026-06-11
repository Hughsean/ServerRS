use std::sync::Arc;

use serde_json::Value;

use crate::domain::agent::{AgentContext, ToolDefinition};
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::llm::ChatMessage;
use crate::domain::memory::{MemoryRepository, NewSummary};
use crate::domain::rag::RAGRepository;
use crate::domain::user::user_profile::UserProfile;
use crate::domain::user::user_profile_repository::UserProfileRepository;
use crate::shared::error::AppError;

/// Trait for repositories that persist / load conversation summaries.
///
/// This is intentionally separate from `ConversationRepository` because
/// summaries are a derived / consolidated artifact, not raw conversation
/// data.  Implementations typically read from the `conversation_summaries`
/// table or an equivalent read-model store.
#[async_trait::async_trait]
pub trait SummaryRepository: Send + Sync {
    /// Load the most recent summary for a conversation, if one exists.
    async fn find_latest_by_conversation(
        &self,
        conversation_id: u64,
    ) -> Result<Option<String>, AppError>;

    /// Persist a new summary.
    async fn save_summary(&self, summary: NewSummary) -> Result<(), AppError>;
}

/// Builder that assembles a fully-populated `AgentContext` for a single turn.
pub struct AgentContextBuilder {
    memory_repo: Arc<dyn MemoryRepository>,
    rag_repo: Arc<dyn RAGRepository>,
    summary_repo: Arc<dyn SummaryRepository>,
    conversation_repo: Arc<dyn ConversationRepository>,
    user_profile_repo: Arc<dyn UserProfileRepository>,
}

impl AgentContextBuilder {
    pub fn new(
        memory_repo: Arc<dyn MemoryRepository>,
        rag_repo: Arc<dyn RAGRepository>,
        summary_repo: Arc<dyn SummaryRepository>,
        conversation_repo: Arc<dyn ConversationRepository>,
        user_profile_repo: Arc<dyn UserProfileRepository>,
    ) -> Self {
        Self {
            memory_repo,
            rag_repo,
            summary_repo,
            conversation_repo,
            user_profile_repo,
        }
    }

    /// Build an `AgentContext` for the given turn.
    ///
    /// 1. Loads the conversation summary (if any).
    /// 2. Recalls semantically relevant memories for the user, using the
    ///    concatenated recent messages as the query.
    /// 3. Retrieves knowledge-base chunks relevant to the user's query (RAG).
    /// 4. Loads the user profile (if any).
    ///
    /// Errors from any of the individual lookups are logged / swallowed so
    /// that a missing profile or empty RAG index does not block the agent.
    pub async fn build(
        &self,
        session_id: String,
        user_id: u64,
        conversation_id: Option<u64>,
        recent_messages: Vec<ChatMessage>,
        user_profile: Option<UserProfile>,
        tools: Vec<ToolDefinition>,
    ) -> AgentContext {
        // ── Summary ────────────────────────────────────────────
        let summary = if let Some(cid) = conversation_id {
            self.summary_repo
                .find_latest_by_conversation(cid)
                .await
                .unwrap_or(None)
        } else {
            None
        };

        // ── Memory recall (semantic search) ────────────────────
        let query_text: String = recent_messages
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        // If there are no user messages yet we derive a generic query
        // from the conversation context.
        let recall_query = if query_text.is_empty() {
            "user conversation context".to_string()
        } else {
            query_text
        };

        let memories = self
            .memory_repo
            .search_by_user(user_id, &recall_query, 10)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|m| format!("[{}] {} (confidence: {:.2})", m.memory_type, m.content, m.confidence))
            .collect();

        // ── RAG retrieval ──────────────────────────────────────
        let rag_query = recent_messages
            .iter()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("")
            .to_string();

        let rag_chunks = if rag_query.is_empty() {
            Vec::new()
        } else {
            self.rag_repo
                .search_by_keyword(&rag_query, 5)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(chunk, _score)| chunk.content)
                .collect()
        };

        // ── User profile (fallback if not provided by caller) ──
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
