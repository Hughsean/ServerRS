use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::application::agent::agent_runtime::{AgentResponse, AgentRuntime};
use crate::domain::conversation::conversation::Conversation;
use crate::domain::conversation::conversation_repository::ConversationRepository;
use crate::domain::tasks::task_event::{ConversationLifecycleTask, TaskEvent, TurnClosedEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;
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
    conv_repo: Arc<dyn ConversationRepository>,
    agent_runtime: Arc<AgentRuntime>,
    user_locks: UserMutexMap,
}

/// Response from a single chat turn.
#[derive(Debug, Clone)]
pub struct ChatTurnResponse {
    pub reply: String,
    pub conversation_id: u64,
}

impl ChatService {
    pub fn new(
        task_publisher: Arc<dyn TaskPublisher>,
        conv_repo: Arc<dyn ConversationRepository>,
        agent_runtime: Arc<AgentRuntime>,
    ) -> Self {
        Self {
            task_publisher,
            conv_repo,
            agent_runtime,
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
    pub async fn open(&self, user_id: u64) -> Result<Conversation, AppError> {
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

        Ok(conversation)
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
        })
    }

    /// Locks the user for admin / lifecycle operations.
    pub fn lock(&self, user_id: u64) -> Arc<Mutex<()>> {
        self.user_lock(user_id)
    }

    /// POST /api/v1/chat/transcript/clear
    pub async fn clear_transcript(&self, user_id: u64) -> Result<(), AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;

        let conv = self.conv_repo.find_single_by_user_id(user_id).await?;
        if let Some(conv) = conv {
            self.conv_repo
                .delete_messages_by_conversation_id(conv.id)
                .await?;
        }
        Ok(())
    }
}
