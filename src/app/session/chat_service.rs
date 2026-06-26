use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::app::agent::agent_runtime::ToolTrace;
use crate::app::agent::agent_runtime::{AgentResponse, AgentRuntime};
use crate::app::memory::memory_service::MemoryService;
use crate::app::rag::vector_index_service::VectorIndexService;
use crate::domain::conversation::conversation::Conversation;
use crate::domain::conversation::conversation_repository::ConversationRepoT;
use crate::domain::tasks::task_event::{ConversationLifecycleTask, TaskEvent, TurnClosedEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::domain::user::user_context_control::{
    ForgetResult, PersonaRebuildResult, PersonaResetResult, PersonaView, TranscriptClearResult,
    UserContextControlRepoT,
};
use crate::shared::error::AppError;

/// Per-user mutex for serializing concurrent requests from the same user.
type UserMutexMap = DashMap<u64, Arc<Mutex<()>>>;

/// ChatService is the primary business entry point for the sessionless,
/// per-user conversation model.
///
/// Flow:
/// 1. Acquire per-user lock
/// 2. find_or_create_for_user(user_id) — single Conversation per user
/// 3. Build AgentContext via PromptBuilder (inside AgentRuntime)
/// 4. Call AgentRuntime::respond
/// 5. Persist user + assistant messages (AgentRuntime handles persistence)
/// 6. Return reply
/// 7. After response closed: emit TurnClosedEvent for post-processing
pub struct ChatService {
    task_publisher: Arc<dyn TaskPublisher>,
    conv_repo: Arc<dyn ConversationRepoT>,
    agent_runtime: Arc<AgentRuntime>,
    memory_service: Arc<MemoryService>,
    context_control_repo: Arc<dyn UserContextControlRepoT>,
    vector_index: Option<Arc<VectorIndexService>>,
    user_locks: UserMutexMap,
}

#[derive(Debug, Clone)]
pub struct ChatOpenResult {
    pub conversation: Conversation,
    pub personalization_enabled: bool,
}

/// Response from a single chat turn.
#[derive(Debug, Clone)]
pub struct ChatTurnResponse {
    pub reply: String,
    pub conversation_id: u64,
    pub tool_calls: Vec<ToolTrace>,
}

impl ChatService {
    pub fn new(
        task_publisher: Arc<dyn TaskPublisher>,
        conv_repo: Arc<dyn ConversationRepoT>,
        agent_runtime: Arc<AgentRuntime>,
        memory_service: Arc<MemoryService>,
        context_control_repo: Arc<dyn UserContextControlRepoT>,
        vector_index: Option<Arc<VectorIndexService>>,
    ) -> Self {
        Self {
            task_publisher,
            conv_repo,
            agent_runtime,
            memory_service,
            context_control_repo,
            vector_index,
            user_locks: DashMap::new(),
        }
    }

    /// Acquire (or create) the per-user mutex.
    fn user_lock(&self, user_id: u64) -> Arc<Mutex<()>> {
        self.user_locks
            .entry(user_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .value()
            .clone()
    }

    /// POST /api/v1/chat/open
    /// Ensure a Conversation exists for this user. Returns the conversation metadata.
    /// Publishes ConversationCreated when the conversation is newly created.
    pub async fn open(&self, user_id: u64) -> Result<ChatOpenResult, AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;

        let conversation = self.conv_repo.find_or_create_for_user(user_id).await?;

        // Heuristic: a brand-new conversation has message_count == 0.
        // Publish ConversationCreated for audit trail.
        if conversation.message_count == 0 {
            let event = TaskEvent::ConversationCreated(ConversationLifecycleTask {
                conversation_id: conversation.id,
                user_id,
            });
            if let Err(e) = self.task_publisher.publish(event).await {
                tracing::warn!(error = %e, "failed to publish ConversationCreated");
            }
        }

        let persona = self.context_control_repo.persona_view(user_id).await?;
        Ok(ChatOpenResult {
            conversation,
            personalization_enabled: persona.personalization_enabled,
        })
    }

    /// POST /api/v1/chat/messages
    /// Process a user message and return the assistant's reply.
    pub async fn send_message(
        &self,
        user_id: u64,
        text: String,
        emotion: Option<String>,
        location: Option<HashMap<String, Value>>,
    ) -> Result<ChatTurnResponse, AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;

        // 1. Ensure conversation exists
        let conversation = self.conv_repo.find_or_create_for_user(user_id).await?;
        let conversation_id = conversation.id;

        // 2. Build location JSON value
        let location_value = location
            .as_ref()
            .and_then(|loc| serde_json::to_value(loc).ok());

        // 3. Call AgentRuntime::respond
        let response: AgentResponse = self
            .agent_runtime
            .respond(
                user_id,
                Some(conversation_id),
                text,
                emotion,
                location_value,
                Vec::new(), // recent_messages — AgentRuntime loads from DB
            )
            .await?;

        // 4. Publish TurnClosedEvent for post-processing
        let event = TaskEvent::TurnClosed(TurnClosedEvent {
            user_id,
            conversation_id,
            user_message_id: response.user_message_id,
            assistant_message_id: response.assistant_message_id,
            closed_at: chrono::Utc::now(),
        });

        if let Err(e) = self.task_publisher.publish(event).await {
            tracing::warn!(error = %e, "failed to publish TurnClosedEvent");
        }

        Ok(ChatTurnResponse {
            reply: response.reply,
            conversation_id,
            tool_calls: response.tool_calls,
        })
    }

    /// Locks the user for admin / lifecycle operations.
    pub fn lock(&self, user_id: u64) -> Arc<Mutex<()>> {
        self.user_lock(user_id)
    }

    pub async fn disable_memory(&self, user_id: u64, memory_id: u64) -> Result<(), AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;
        self.memory_service.disable(memory_id, user_id).await
    }

    pub async fn persona(&self, user_id: u64) -> Result<PersonaView, AppError> {
        self.context_control_repo.persona_view(user_id).await
    }

    pub async fn reset_persona(&self, user_id: u64) -> Result<PersonaResetResult, AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;
        self.context_control_repo.reset_persona(user_id).await
    }

    pub async fn rebuild_persona(&self, user_id: u64) -> Result<PersonaRebuildResult, AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;
        self.context_control_repo.rebuild_persona(user_id).await
    }

    pub async fn clear_transcript(&self, user_id: u64) -> Result<TranscriptClearResult, AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;

        let result = self.context_control_repo.clear_transcript(user_id).await?;
        if let Some(vector_index) = &self.vector_index {
            for summary_id in &result.summary_ids {
                if let Err(error) = vector_index.delete_summary_index(*summary_id).await {
                    tracing::warn!(
                        summary_id,
                        %error,
                        "failed to immediately delete cleared summary vector"
                    );
                }
            }
        }
        Ok(result)
    }

    pub async fn forget(&self, user_id: u64) -> Result<ForgetResult, AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;

        let result = self.context_control_repo.forget(user_id).await?;
        if let Some(vector_index) = &self.vector_index {
            for summary_id in &result.summary_ids {
                if let Err(error) = vector_index.delete_summary_index(*summary_id).await {
                    tracing::warn!(
                        summary_id,
                        %error,
                        "failed to immediately delete forgotten summary vector"
                    );
                }
            }
            for memory_id in &result.memory_ids {
                if let Err(error) = vector_index.delete_memory_index(*memory_id).await {
                    tracing::warn!(
                        memory_id,
                        %error,
                        "failed to immediately delete forgotten memory vector"
                    );
                }
            }
        }
        Ok(result)
    }
}
