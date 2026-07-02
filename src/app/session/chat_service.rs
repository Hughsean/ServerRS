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
use crate::domain::conversation::conversation_message::ConversationMessage;
use crate::domain::conversation::conversation_repo::ConversationRepoT;
use crate::domain::llm::ChatMessage;
use crate::domain::tasks::task_event::{ConversationLifecycleTask, TaskEvent, TurnClosedEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::domain::user::user_context_control::{
    ForgetResult, PersonaRebuildResult, PersonaResetResult, PersonaView, TranscriptClearResult,
    UserContextControlRepoT,
};
use crate::shared::error::AppError;

/// 每个用户独立的 mutex，用于串行化同一用户的并发请求。
type UserMutexMap = DashMap<u64, Arc<Mutex<()>>>;

const FALLBACK_RECENT_MESSAGE_LIMIT: u64 = 100;

/// ChatService 是无会话、按用户维护对话模型的主要业务入口。
///
/// Flow:
/// 1. 获取 per-user lock
/// 2. find_or_create_for_user(user_id) —— 每个用户对应一个 Conversation
/// 3. 通过 PromptBuilder 构建 AgentContext（在 AgentRuntime 内部完成）
/// 4. 调用 AgentRuntime::respond
/// 5. 持久化 user + assistant messages（由 AgentRuntime 处理）
/// 6. 返回 reply
/// 7. 响应结束后，发送 TurnClosedEvent 供后处理使用
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

/// 单轮聊天的响应结果。
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

    /// 获取（或创建）当前用户对应的 mutex。
    fn user_lock(&self, user_id: u64) -> Arc<Mutex<()>> {
        self.user_locks
            .entry(user_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .value()
            .clone()
    }

    /// POST /api/v1/chat/open
    /// 确保该用户存在一个 Conversation，并返回 conversation metadata。
    /// 如果是新建的 conversation，则发布 ConversationCreated。
    pub async fn open(&self, user_id: u64) -> Result<ChatOpenResult, AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;

        let conversation = self.conv_repo.find_or_create_for_user(user_id).await?;

        // 启发式判断：全新的 conversation 会有 message_count == 0。
        // 发布 ConversationCreated，便于审计追踪。
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
    /// 处理用户消息，并返回 assistant 的回复。
    pub async fn send_message(
        &self,
        user_id: u64,
        text: String,
        emotion: Option<String>,
        location: Option<HashMap<String, Value>>,
    ) -> Result<ChatTurnResponse, AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;

        // 1. 确保 conversation 存在
        let conversation = self.conv_repo.find_or_create_for_user(user_id).await?;
        let conversation_id = conversation.id;

        // 2. 构建 location 的 JSON value
        let location_value = location
            .as_ref()
            .and_then(|loc| serde_json::to_value(loc).ok());

        // 3. 加载当前轮之前最近持久化的对话。runtime 会追加当前用户消息，
        // 并应用最终的上下文长度限制。
        let recent_messages = self.load_recent_chat_messages(conversation_id).await?;

        // 4. 调用 AgentRuntime::respond
        let response: AgentResponse = self
            .agent_runtime
            .respond(
                user_id,
                Some(conversation_id),
                text,
                emotion,
                location_value,
                recent_messages,
            )
            .await?;

        // 5. 发布 TurnClosedEvent，供后处理使用
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

    /// 为 admin / lifecycle 操作锁定该用户。
    pub fn lock(&self, user_id: u64) -> Arc<Mutex<()>> {
        self.user_lock(user_id)
    }

    pub async fn disable_memory(&self, user_id: u64, memory_id: u64) -> Result<(), AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;
        self.memory_service.disable(memory_id, user_id).await
    }

    pub async fn persona(&self, user_id: u64) -> Result<PersonaView, AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;
        self.context_control_repo
            .refresh_persona_if_stale(user_id)
            .await?;
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

    async fn load_recent_chat_messages(
        &self,
        conversation_id: u64,
    ) -> Result<Vec<ChatMessage>, AppError> {
        let limit = match self.agent_runtime.max_context_messages() {
            0 => FALLBACK_RECENT_MESSAGE_LIMIT,
            1 => return Ok(Vec::new()),
            value => value.saturating_sub(1) as u64,
        };
        let messages = self
            .conv_repo
            .find_messages_before(conversation_id, None, limit)
            .await?;
        Ok(messages
            .into_iter()
            .filter_map(conversation_message_to_chat_message)
            .collect())
    }
}

fn conversation_message_to_chat_message(message: ConversationMessage) -> Option<ChatMessage> {
    if !matches!(
        message.sender_role.as_str(),
        "system" | "user" | "assistant"
    ) {
        return None;
    }
    if message.message_type != "text" {
        return None;
    }

    let content = conversation_message_text(&message);
    if content.trim().is_empty() {
        return None;
    }

    Some(ChatMessage {
        role: message.sender_role,
        content,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    })
}

fn conversation_message_text(message: &ConversationMessage) -> String {
    let Ok(value) = serde_json::from_str::<Value>(&message.content) else {
        return message.content.clone();
    };

    let mut text = value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| message.content.clone());

    if message.sender_role == "user" {
        if let Some(emotion) = value
            .get("emotion")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|emotion| !emotion.is_empty())
        {
            text = format!("{text}\n\n[user emotion: {emotion}]");
        }
    }

    text
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn message(role: &str, message_type: &str, content: &str) -> ConversationMessage {
        ConversationMessage {
            id: 1,
            conversation_id: 1,
            sender_role: role.into(),
            sender_user_id: None,
            message_type: message_type.into(),
            content: content.into(),
            token_count: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn converts_persisted_text_json_to_chat_message() {
        let converted = conversation_message_to_chat_message(message(
            "assistant",
            "text",
            r#"{"text":"你好"}"#,
        ))
        .unwrap();

        assert_eq!(converted.role, "assistant");
        assert_eq!(converted.content, "你好");
    }

    #[test]
    fn preserves_historical_user_emotion() {
        let converted = conversation_message_to_chat_message(message(
            "user",
            "text",
            r#"{"text":"今天很累","emotion":"sad"}"#,
        ))
        .unwrap();

        assert_eq!(converted.content, "今天很累\n\n[user emotion: sad]");
    }

    #[test]
    fn skips_non_dialogue_or_non_text_messages() {
        assert!(conversation_message_to_chat_message(message("plugin", "text", "x")).is_none());
        assert!(conversation_message_to_chat_message(message("user", "image", "x")).is_none());
    }
}
