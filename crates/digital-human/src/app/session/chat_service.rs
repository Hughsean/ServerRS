use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::app::agent::agent_runtime::{
    AgentResponse, AgentRunOutcome, AgentRuntime, AgentSuspension, ToolTrace,
};
use crate::app::agent::chat_state::{ChatResponseMode, ChatResumeInput, ToolApprovalDecision};
use crate::app::agent::graph::{CheckpointId, RunId};
use crate::app::memory::memory_service::MemoryService;
use crate::app::rag::vector_index_service::VectorIndexService;
use crate::app::session::chat_approval_service::ChatApprovalService;
use crate::domain::agent::{
    ChatApprovalDecision, ChatApprovalDecisionEvent, PendingApprovalPage, PendingChatApproval,
};
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
    approvals: Arc<ChatApprovalService>,
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
    /// 本轮运行的 RunId；恢复完成时保留暂停前的原始 RunId。
    pub run_id: RunId,
}

#[derive(Debug, Clone)]
pub enum ChatTurnOutcome {
    Completed(ChatTurnResponse),
    Suspended(AgentSuspension),
}

impl ChatService {
    pub fn new(
        task_publisher: Arc<dyn TaskPublisher>,
        conv_repo: Arc<dyn ConversationRepoT>,
        agent_runtime: Arc<AgentRuntime>,
        memory_service: Arc<MemoryService>,
        context_control_repo: Arc<dyn UserContextControlRepoT>,
        vector_index: Option<Arc<VectorIndexService>>,
        approvals: Arc<ChatApprovalService>,
    ) -> Self {
        Self {
            task_publisher,
            conv_repo,
            agent_runtime,
            memory_service,
            context_control_repo,
            vector_index,
            approvals,
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
        match self
            .send_message_checkpointed(user_id, text, emotion, location)
            .await?
        {
            ChatTurnOutcome::Completed(response) => Ok(response),
            ChatTurnOutcome::Suspended(_) => Err(AppError::Conflict(
                "该请求已暂停，调用方需要使用 Checkpoint 响应协议".into(),
            )),
        }
    }

    /// 处理支持暂停的用户消息。只有完整结束的轮次才发布 TurnClosedEvent。
    pub async fn send_message_checkpointed(
        &self,
        user_id: u64,
        text: String,
        emotion: Option<String>,
        location: Option<HashMap<String, Value>>,
    ) -> Result<ChatTurnOutcome, AppError> {
        self.send_message_checkpointed_with_mode(
            user_id,
            text,
            emotion,
            location,
            ChatResponseMode::Text,
        )
        .await
    }

    pub async fn send_message_checkpointed_with_mode(
        &self,
        user_id: u64,
        text: String,
        emotion: Option<String>,
        location: Option<HashMap<String, Value>>,
        response_mode: ChatResponseMode,
    ) -> Result<ChatTurnOutcome, AppError> {
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

        // 4. 调用可持久暂停的 Agent Runtime。
        let outcome = self
            .agent_runtime
            .respond_checkpointed(
                user_id,
                Some(conversation_id),
                text,
                emotion,
                location_value,
                recent_messages,
                response_mode,
            )
            .await?;

        self.map_runtime_outcome(user_id, outcome).await
    }

    /// 恢复一个待审批的 Chat Checkpoint。
    ///
    /// 恢复成功后以最佳努力记录审批决策审计；审计失败不影响已完成的
    /// Resume 结果，也不会触发工具重放。
    pub async fn resume_checkpoint(
        &self,
        user_id: u64,
        checkpoint_id: CheckpointId,
        approval_id: uuid::Uuid,
        decision: ToolApprovalDecision,
    ) -> Result<ChatTurnOutcome, AppError> {
        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;
        let outcome = self
            .agent_runtime
            .resume(
                checkpoint_id,
                ChatResumeInput {
                    user_id,
                    approval_id,
                    decision,
                },
            )
            .await?;
        let outcome = self.map_runtime_outcome(user_id, outcome).await?;

        // 审计只记录最小字段；run_id 来自恢复结果，保持原始 RunId。
        let (conversation_id, run_id) = match &outcome {
            ChatTurnOutcome::Completed(response) => (response.conversation_id, response.run_id),
            ChatTurnOutcome::Suspended(suspension) => {
                (suspension.conversation_id, suspension.run_id)
            }
        };
        self.approvals
            .audit_decision(ChatApprovalDecisionEvent {
                user_id,
                conversation_id,
                checkpoint_id,
                run_id,
                approval_id,
                decision: match decision {
                    ToolApprovalDecision::Approve => ChatApprovalDecision::Approve,
                    ToolApprovalDecision::Reject => ChatApprovalDecision::Reject,
                },
            })
            .await;
        Ok(outcome)
    }

    /// 列出当前用户的待审批 Checkpoint（非消费式查询）。
    pub async fn list_pending_approvals(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        limit: Option<u32>,
    ) -> Result<PendingApprovalPage, AppError> {
        self.approvals
            .list_pending(user_id, conversation_id, limit)
            .await
    }

    /// 读取当前用户的单个待审批 Checkpoint（非消费式查询）。
    pub async fn get_pending_approval(
        &self,
        user_id: u64,
        checkpoint_id: CheckpointId,
    ) -> Result<PendingChatApproval, AppError> {
        self.approvals.get_pending(user_id, checkpoint_id).await
    }

    async fn map_runtime_outcome(
        &self,
        user_id: u64,
        outcome: AgentRunOutcome,
    ) -> Result<ChatTurnOutcome, AppError> {
        match outcome {
            AgentRunOutcome::Completed(response) => Ok(ChatTurnOutcome::Completed(
                self.complete_turn(user_id, response).await,
            )),
            AgentRunOutcome::Suspended(suspension) => Ok(ChatTurnOutcome::Suspended(suspension)),
        }
    }

    async fn complete_turn(&self, user_id: u64, response: AgentResponse) -> ChatTurnResponse {
        let conversation_id = response.conversation_id;

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

        ChatTurnResponse {
            reply: response.reply,
            conversation_id,
            tool_calls: response.tool_calls,
            run_id: response.run_id,
        }
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

/// resume 审批审计挂载的端到端测试（内存 Checkpoint + 脚本化 LLM）。
#[cfg(test)]
mod resume_audit_tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::Utc;

    use super::*;
    use crate::app::agent::agent_runtime::AgentRuntimeSettings;
    use crate::app::agent::chat_effect::TurnWriterT;
    use crate::app::agent::chat_graph::{ChatAgentGraph, ChatAgentGraphDeps};
    use crate::app::agent::chat_state::{ChatSuspendData, PersistedTurn, ToolApprovalRequest};
    use crate::app::agent::graph::InMemoryCheckpointStore;
    use crate::app::agent::memory_extraction::{
        MemoryExtractionDispatch, MemoryExtractionRequest, MemoryExtractionSchedulerT,
    };
    use crate::app::agent::nodes::{ChatContextProviderT, ChatContextRequest, LoadedChatContext};
    use crate::app::agent::tool::AgentTool;
    use crate::app::memory::memory_extractor::MemoryExtractor;
    use crate::app::memory::memory_extractor::test_utils::MockLlm;
    use crate::app::session::chat_approval_service::ChatApprovalService;
    use crate::domain::agent::{
        AgentContext, AgentEvent, AgentEventRepoT, ChatApprovalQueryT, NewAgentEvent,
    };
    use crate::domain::conversation::conversation::NewConversation;
    use crate::domain::conversation::conversation_message::NewConversationMessage;
    use crate::domain::llm::{
        ChatCompletionRequest, ChatCompletionResponse, LlmError, LlmProvider, ToolCall,
        ToolDefinition,
    };
    use crate::domain::memory::{MemoryRepoT, NewMemory, NewMemoryEvidence, UserMemory};
    use crate::domain::tasks::task_event::TaskEvent;
    use crate::domain::user::user_context_control::{
        ForgetResult, PersonaRebuildResult, PersonaResetResult, PersonaView, TranscriptClearResult,
    };

    fn stub_err<T>() -> Result<T, AppError> {
        Err(AppError::Internal("test stub repo".into()))
    }

    struct NoopTaskPublisher;

    #[async_trait]
    impl TaskPublisher for NoopTaskPublisher {
        async fn publish(&self, _event: TaskEvent) -> Result<(), AppError> {
            Ok(())
        }
    }

    struct StubConversationRepo;

    #[async_trait]
    impl ConversationRepoT for StubConversationRepo {
        async fn find_by_id(&self, _id: u64) -> Result<Option<Conversation>, AppError> {
            stub_err()
        }
        async fn find_by_user_id(&self, _user_id: u64) -> Result<Vec<Conversation>, AppError> {
            stub_err()
        }
        async fn find_or_create_for_user(&self, user_id: u64) -> Result<Conversation, AppError> {
            Ok(Conversation {
                id: 9,
                user_id,
                title: None,
                last_message_at: None,
                message_count: 0,
                created_at: Utc::now(),
            })
        }
        async fn find_single_by_user_id(
            &self,
            _user_id: u64,
        ) -> Result<Option<Conversation>, AppError> {
            stub_err()
        }
        async fn save(&self, _conv: NewConversation) -> Result<Conversation, AppError> {
            stub_err()
        }
        async fn update_title(&self, _id: u64, _title: &str) -> Result<(), AppError> {
            stub_err()
        }
        async fn touch_and_incr(&self, _id: u64, _inc: u64) -> Result<(), AppError> {
            stub_err()
        }
        async fn delete_by_id(&self, _id: u64) -> Result<bool, AppError> {
            stub_err()
        }
        async fn save_message(
            &self,
            _msg: NewConversationMessage,
        ) -> Result<ConversationMessage, AppError> {
            stub_err()
        }
        async fn find_messages_by_conversation_id(
            &self,
            _conversation_id: u64,
        ) -> Result<Vec<ConversationMessage>, AppError> {
            stub_err()
        }
        async fn delete_messages_by_conversation_id(
            &self,
            _conversation_id: u64,
        ) -> Result<u64, AppError> {
            stub_err()
        }
        async fn find_messages_before(
            &self,
            _conversation_id: u64,
            _before_id: Option<u64>,
            _limit: u64,
        ) -> Result<Vec<ConversationMessage>, AppError> {
            Ok(Vec::new())
        }
        async fn find_messages_since(
            &self,
            _conversation_id: u64,
            _since_id: u64,
        ) -> Result<Vec<ConversationMessage>, AppError> {
            stub_err()
        }
        async fn find_messages_by_ids(
            &self,
            _conversation_id: u64,
            _message_ids: &[u64],
        ) -> Result<Vec<ConversationMessage>, AppError> {
            stub_err()
        }
        async fn save_turn_atomic(
            &self,
            _conversation_id: u64,
            _user_id: u64,
            _user_msg: NewConversationMessage,
            _assistant_msg: NewConversationMessage,
        ) -> Result<(ConversationMessage, ConversationMessage), AppError> {
            stub_err()
        }
    }

    struct StubContextControlRepo;

    #[async_trait]
    impl UserContextControlRepoT for StubContextControlRepo {
        async fn persona_view(&self, _user_id: u64) -> Result<PersonaView, AppError> {
            stub_err()
        }
        async fn refresh_persona_if_stale(
            &self,
            _user_id: u64,
        ) -> Result<Option<PersonaRebuildResult>, AppError> {
            stub_err()
        }
        async fn reset_persona(&self, _user_id: u64) -> Result<PersonaResetResult, AppError> {
            stub_err()
        }
        async fn rebuild_persona(&self, _user_id: u64) -> Result<PersonaRebuildResult, AppError> {
            stub_err()
        }
        async fn clear_transcript(&self, _user_id: u64) -> Result<TranscriptClearResult, AppError> {
            stub_err()
        }
        async fn forget(&self, _user_id: u64) -> Result<ForgetResult, AppError> {
            stub_err()
        }
    }

    struct StubMemoryRepo;

    #[async_trait]
    impl MemoryRepoT for StubMemoryRepo {
        async fn save_memory_with_evidence(
            &self,
            _memory: NewMemory,
            _evidence: NewMemoryEvidence,
        ) -> Result<UserMemory, AppError> {
            stub_err()
        }
        async fn reinforce_memory_with_evidence(
            &self,
            _memory_id: u64,
            _evidence: NewMemoryEvidence,
            _confidence: f64,
        ) -> Result<UserMemory, AppError> {
            stub_err()
        }
        async fn save_contradicting_memory_with_evidence(
            &self,
            _memory: NewMemory,
            _evidence: NewMemoryEvidence,
            _contradicted_memory_id: u64,
        ) -> Result<UserMemory, AppError> {
            stub_err()
        }
        async fn find_by_id(&self, _memory_id: u64) -> Result<Option<UserMemory>, AppError> {
            stub_err()
        }
        async fn find_by_user_id(
            &self,
            _user_id: u64,
            _status: Option<i8>,
        ) -> Result<Vec<UserMemory>, AppError> {
            stub_err()
        }
        async fn search_by_user(
            &self,
            _user_id: u64,
            _query: &str,
            _top_k: u32,
        ) -> Result<Vec<UserMemory>, AppError> {
            stub_err()
        }
        async fn update_memory(
            &self,
            _memory_id: u64,
            _content: Option<String>,
            _confidence: Option<f64>,
        ) -> Result<UserMemory, AppError> {
            stub_err()
        }
        async fn disable_memory(&self, _memory_id: u64) -> Result<(), AppError> {
            stub_err()
        }
        async fn delete_memory(&self, _memory_id: u64) -> Result<bool, AppError> {
            stub_err()
        }
        async fn find_memories_by_conversation(
            &self,
            _conversation_id: u64,
        ) -> Result<Vec<UserMemory>, AppError> {
            stub_err()
        }
        async fn update_memory_index_metadata(
            &self,
            _memory_id: u64,
            _vector_id: String,
            _embedding_provider: String,
            _embedding_model: String,
            _embedding_dimension: u32,
        ) -> Result<(), AppError> {
            stub_err()
        }
        async fn touch_memory_access(&self, _memory_id: u64) -> Result<(), AppError> {
            stub_err()
        }
        async fn find_by_memory_key(
            &self,
            _user_id: u64,
            _memory_key: &str,
        ) -> Result<Option<UserMemory>, AppError> {
            stub_err()
        }
        async fn list_indexable_memories(
            &self,
            _user_id: Option<u64>,
            _limit: u64,
        ) -> Result<Vec<UserMemory>, AppError> {
            stub_err()
        }
    }

    struct FakeEventRepo;

    #[async_trait]
    impl AgentEventRepoT for FakeEventRepo {
        async fn log_event(&self, event: NewAgentEvent) -> AgentEvent {
            AgentEvent {
                event_id: 1,
                user_id: event.user_id,
                conversation_id: event.conversation_id,
                trace_id: None,
                event_type: event.event_type,
                tool_name: event.tool_name,
                payload: event.payload,
                created_at: Utc::now(),
            }
        }
    }

    struct FakeContextProvider;

    #[async_trait]
    impl ChatContextProviderT for FakeContextProvider {
        async fn load(&self, request: ChatContextRequest) -> Result<LoadedChatContext, AppError> {
            Ok(LoadedChatContext {
                context: AgentContext {
                    user_id: request.user_id,
                    conversation_id: Some(request.conversation_id),
                    recent_messages: request.recent_messages,
                    summary: None,
                    memories: vec![],
                    rag_chunks: vec![],
                    fresh_chunks: vec![],
                    user_profile: None,
                    tools: request.tools,
                    location: request.location,
                },
                context_version: 23,
            })
        }
    }

    struct FakeTurnWriter;

    #[async_trait]
    impl TurnWriterT for FakeTurnWriter {
        async fn save_turn_atomic(
            &self,
            _conversation_id: u64,
            _user_id: u64,
            _user: NewConversationMessage,
            _assistant: NewConversationMessage,
        ) -> Result<PersistedTurn, AppError> {
            Ok(PersistedTurn::new(101, 102))
        }
    }

    struct NoopMemoryScheduler;

    impl MemoryExtractionSchedulerT for NoopMemoryScheduler {
        fn schedule(&self, _request: MemoryExtractionRequest) -> MemoryExtractionDispatch {
            MemoryExtractionDispatch::Scheduled
        }
    }

    struct ScriptedToolLlm {
        responses: Mutex<VecDeque<ChatCompletionResponse>>,
    }

    impl ScriptedToolLlm {
        fn tool_call() -> ChatCompletionResponse {
            ChatCompletionResponse {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-approval-1".into(),
                    name: "controlled_tool".into(),
                    arguments: serde_json::json!({"value": 7}),
                }],
                finish_reason: "tool_calls".into(),
                usage: None,
            }
        }

        fn text(content: &str) -> ChatCompletionResponse {
            ChatCompletionResponse {
                content: content.into(),
                tool_calls: vec![],
                finish_reason: "stop".into(),
                usage: None,
            }
        }

        fn single_suspend() -> Self {
            Self {
                responses: Mutex::new(VecDeque::from([
                    Self::tool_call(),
                    Self::text("approval flow complete"),
                ])),
            }
        }

        fn double_suspend() -> Self {
            Self {
                responses: Mutex::new(VecDeque::from([
                    Self::tool_call(),
                    Self::tool_call(),
                    Self::text("second approval complete"),
                ])),
            }
        }

        fn next(&self) -> Result<ChatCompletionResponse, LlmError> {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| LlmError::InvalidResponse("script exhausted".into()))
        }
    }

    #[async_trait]
    impl LlmProvider for ScriptedToolLlm {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, LlmError> {
            self.next()
        }

        async fn chat_with_tools(
            &self,
            _request: ChatCompletionRequest,
            _tools: Vec<ToolDefinition>,
        ) -> Result<ChatCompletionResponse, LlmError> {
            self.next()
        }
    }

    #[derive(Default)]
    struct RecordingTool {
        calls: Mutex<Vec<serde_json::Value>>,
    }

    #[async_trait]
    impl AgentTool for RecordingTool {
        fn name(&self) -> &str {
            "controlled_tool"
        }

        fn description(&self) -> &str {
            "test controlled tool"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _context: &AgentContext,
            args: serde_json::Value,
        ) -> Result<String, AppError> {
            self.calls.lock().unwrap().push(args);
            Ok("controlled result".into())
        }
    }

    struct UnusedApprovalQuery;

    #[async_trait]
    impl ChatApprovalQueryT for UnusedApprovalQuery {
        async fn list_pending_approvals(
            &self,
            _user_id: u64,
            _conversation_id: Option<u64>,
            _limit: u32,
        ) -> Result<PendingApprovalPage, AppError> {
            stub_err()
        }

        async fn get_pending_approval(
            &self,
            _user_id: u64,
            _checkpoint_id: CheckpointId,
        ) -> Result<Option<PendingChatApproval>, AppError> {
            stub_err()
        }
    }

    #[derive(Default)]
    struct RecordingApprovalAudit {
        events: Mutex<Vec<ChatApprovalDecisionEvent>>,
        fail: bool,
    }

    #[async_trait]
    impl crate::domain::agent::ChatApprovalAuditT for RecordingApprovalAudit {
        async fn record_decision(&self, event: ChatApprovalDecisionEvent) -> Result<(), AppError> {
            if self.fail {
                return Err(AppError::Infrastructure("audit store down".into()));
            }
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    struct ApprovalHarness {
        service: ChatService,
        audit: Arc<RecordingApprovalAudit>,
        tool: Arc<RecordingTool>,
    }

    fn approval_service(llm: ScriptedToolLlm, audit_fail: bool) -> ApprovalHarness {
        let audit = Arc::new(RecordingApprovalAudit {
            fail: audit_fail,
            ..RecordingApprovalAudit::default()
        });
        let approvals = Arc::new(ChatApprovalService::new(
            Arc::new(UnusedApprovalQuery),
            audit.clone(),
        ));
        let tool = Arc::new(RecordingTool::default());
        let settings = AgentRuntimeSettings {
            approval_required_tools: vec!["controlled_tool".into()],
            memory_extraction_async: false,
            max_tool_depth: 3,
            temperature: 0.0,
            top_p: 1.0,
            enable_reasoning: false,
            ..AgentRuntimeSettings::default()
        };
        let graph = ChatAgentGraph::new(ChatAgentGraphDeps {
            llm: Arc::new(llm),
            event_repo: Arc::new(FakeEventRepo),
            context_provider: Arc::new(FakeContextProvider),
            turn_writer: Arc::new(FakeTurnWriter),
            memory_extraction_scheduler: Arc::new(NoopMemoryScheduler),
            checkpoint_store: Arc::new(InMemoryCheckpointStore::new()),
            tools: vec![tool.clone()],
            settings,
        })
        .expect("test Chat Agent graph must compile");
        let agent_runtime = Arc::new(AgentRuntime::from_graph(graph, 10));
        let memory_service = Arc::new(MemoryService::new(
            Arc::new(StubMemoryRepo),
            Arc::new(MemoryExtractor::new(Arc::new(MockLlm))),
        ));
        let service = ChatService::new(
            Arc::new(NoopTaskPublisher),
            Arc::new(StubConversationRepo),
            agent_runtime,
            memory_service,
            Arc::new(StubContextControlRepo),
            None,
            approvals,
        );
        ApprovalHarness {
            service,
            audit,
            tool,
        }
    }

    async fn suspend_turn(service: &ChatService) -> AgentSuspension {
        match service
            .send_message_checkpointed(7, "hello".into(), None, None)
            .await
            .expect("send message")
        {
            ChatTurnOutcome::Suspended(suspension) => suspension,
            ChatTurnOutcome::Completed(_) => panic!("controlled tool should suspend"),
        }
    }

    fn approval_id_of(suspension: &AgentSuspension) -> uuid::Uuid {
        match &suspension.data {
            ChatSuspendData::ToolApproval(ToolApprovalRequest { approval_id, .. }) => *approval_id,
        }
    }

    #[tokio::test]
    async fn approve_resume_records_decision_audit_with_original_run_id() {
        let harness = approval_service(ScriptedToolLlm::single_suspend(), false);
        let suspension = suspend_turn(&harness.service).await;
        let approval_id = approval_id_of(&suspension);

        let outcome = harness
            .service
            .resume_checkpoint(
                7,
                suspension.checkpoint_id,
                approval_id,
                ToolApprovalDecision::Approve,
            )
            .await
            .expect("resume succeeds");

        let completed = match outcome {
            ChatTurnOutcome::Completed(response) => response,
            ChatTurnOutcome::Suspended(_) => panic!("approved resume should complete"),
        };
        assert_eq!(completed.run_id, suspension.run_id);
        assert_eq!(harness.tool.calls.lock().unwrap().len(), 1);

        let events = harness.audit.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.user_id, 7);
        assert_eq!(event.conversation_id, 9);
        assert_eq!(event.checkpoint_id, suspension.checkpoint_id);
        assert_eq!(event.run_id, suspension.run_id);
        assert_eq!(event.approval_id, approval_id);
        assert_eq!(event.decision, ChatApprovalDecision::Approve);
    }

    #[tokio::test]
    async fn reject_resume_records_reject_and_never_runs_the_tool() {
        let harness = approval_service(ScriptedToolLlm::single_suspend(), false);
        let suspension = suspend_turn(&harness.service).await;
        let approval_id = approval_id_of(&suspension);

        let outcome = harness
            .service
            .resume_checkpoint(
                7,
                suspension.checkpoint_id,
                approval_id,
                ToolApprovalDecision::Reject,
            )
            .await
            .expect("reject resume succeeds");

        assert!(matches!(outcome, ChatTurnOutcome::Completed(_)));
        assert!(harness.tool.calls.lock().unwrap().is_empty());

        let events = harness.audit.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].decision, ChatApprovalDecision::Reject);
        assert_eq!(events[0].checkpoint_id, suspension.checkpoint_id);
    }

    #[tokio::test]
    async fn audit_failure_does_not_fail_the_completed_resume() {
        let harness = approval_service(ScriptedToolLlm::single_suspend(), true);
        let suspension = suspend_turn(&harness.service).await;
        let approval_id = approval_id_of(&suspension);

        let outcome = harness
            .service
            .resume_checkpoint(
                7,
                suspension.checkpoint_id,
                approval_id,
                ToolApprovalDecision::Approve,
            )
            .await
            .expect("audit failure must not fail resume");

        assert!(matches!(outcome, ChatTurnOutcome::Completed(_)));
        assert_eq!(harness.tool.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn rejected_resume_input_records_no_audit() {
        let harness = approval_service(ScriptedToolLlm::single_suspend(), false);
        let suspension = suspend_turn(&harness.service).await;

        let error = harness
            .service
            .resume_checkpoint(
                7,
                suspension.checkpoint_id,
                uuid::Uuid::new_v4(),
                ToolApprovalDecision::Approve,
            )
            .await
            .unwrap_err();

        assert!(matches!(error, AppError::Conflict(_)));
        assert!(harness.tool.calls.lock().unwrap().is_empty());
        assert!(harness.audit.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_resumes_have_a_single_winner_and_one_audit() {
        let harness = approval_service(ScriptedToolLlm::single_suspend(), false);
        let suspension = suspend_turn(&harness.service).await;
        let approval_id = approval_id_of(&suspension);
        let service = Arc::new(harness.service);
        let audit = harness.audit.clone();
        let tool = harness.tool.clone();

        let (first, second) = {
            let service_a = service.clone();
            let service_b = service.clone();
            tokio::join!(
                service_a.resume_checkpoint(
                    7,
                    suspension.checkpoint_id,
                    approval_id,
                    ToolApprovalDecision::Approve,
                ),
                service_b.resume_checkpoint(
                    7,
                    suspension.checkpoint_id,
                    approval_id,
                    ToolApprovalDecision::Approve,
                )
            )
        };

        // 竞争恢复只有一个成功；败者不得再次执行工具。
        let outcomes = [first, second];
        let successes = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
        assert_eq!(successes, 1, "并发 Resume 必须只有一个成功: {outcomes:?}");
        let winner = outcomes
            .into_iter()
            .find_map(Result::ok)
            .expect("exactly one winner");
        assert!(matches!(winner, ChatTurnOutcome::Completed(_)));

        assert_eq!(tool.calls.lock().unwrap().len(), 1);
        assert_eq!(audit.events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn resume_that_suspends_again_audits_the_consumed_checkpoint() {
        let harness = approval_service(ScriptedToolLlm::double_suspend(), false);
        let first = suspend_turn(&harness.service).await;
        let first_approval_id = approval_id_of(&first);

        let outcome = harness
            .service
            .resume_checkpoint(
                7,
                first.checkpoint_id,
                first_approval_id,
                ToolApprovalDecision::Approve,
            )
            .await
            .expect("first resume succeeds");

        let second = match outcome {
            ChatTurnOutcome::Suspended(suspension) => suspension,
            ChatTurnOutcome::Completed(_) => panic!("second controlled tool should suspend again"),
        };
        assert_ne!(second.checkpoint_id, first.checkpoint_id);
        assert_eq!(second.run_id, first.run_id);

        let events = harness.audit.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].checkpoint_id, first.checkpoint_id);
        assert_eq!(events[0].run_id, first.run_id);
        drop(events);

        let outcome = harness
            .service
            .resume_checkpoint(
                7,
                second.checkpoint_id,
                approval_id_of(&second),
                ToolApprovalDecision::Reject,
            )
            .await
            .expect("second resume succeeds");
        assert!(matches!(outcome, ChatTurnOutcome::Completed(_)));

        let events = harness.audit.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].checkpoint_id, second.checkpoint_id);
        assert_eq!(events[1].decision, ChatApprovalDecision::Reject);
        // 第一次 approve 执行过一次工具；第二次 reject 不得再次执行。
        assert_eq!(harness.tool.calls.lock().unwrap().len(), 1);
    }
}
