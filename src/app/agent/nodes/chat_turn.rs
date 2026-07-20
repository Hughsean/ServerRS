use crate::app::agent::chat_state::{ChatTurnState, ChatTurnUpdate, PersistedTurn};
use crate::app::agent::graph::{
    AgentNode, NodeError, NodeErrorKind, NodeId, NodeResult, RunContext, UsageDelta,
};
use crate::app::agent::response::{fallback_reply, normalize_final_content};
use crate::domain::agent::{AgentMessage, AgentOutcome, AgentState, AgentUpdate};
use crate::domain::conversation::conversation_message::NewConversationMessage;
use crate::domain::conversation::conversation_repo::ConversationRepoT;
use crate::shared::error::AppError;
use async_trait::async_trait;
use std::sync::Arc;

pub struct PrepareTurnNode {
    id: NodeId,
    max_context_messages: usize,
}

impl PrepareTurnNode {
    pub fn new(id: NodeId, max_context_messages: usize) -> Self {
        Self {
            id,
            max_context_messages,
        }
    }
}

#[async_trait]
impl AgentNode<ChatTurnState> for PrepareTurnNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<ChatTurnState>,
        _context: &RunContext,
    ) -> Result<NodeResult<ChatTurnUpdate>, NodeError> {
        let turn = state.business();
        let messages = prepare_recent_messages(
            turn.recent_messages().to_vec(),
            turn.user_message(),
            turn.emotion(),
            self.max_context_messages,
        );
        Ok(NodeResult::new(
            vec![AgentUpdate::Business(ChatTurnUpdate::SetRecentMessages(
                messages,
            ))],
            UsageDelta::default(),
        ))
    }
}

pub struct NormalizeReplyNode {
    id: NodeId,
}

impl NormalizeReplyNode {
    pub fn new(id: NodeId) -> Self {
        Self { id }
    }
}

#[async_trait]
impl AgentNode<ChatTurnState> for NormalizeReplyNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<ChatTurnState>,
        _context: &RunContext,
    ) -> Result<NodeResult<ChatTurnUpdate>, NodeError> {
        let content = state
            .messages()
            .iter()
            .rev()
            .find_map(|message| match message {
                AgentMessage::Assistant { content, .. } => Some(content.clone()),
                _ => None,
            })
            .unwrap_or_else(fallback_reply);
        Ok(NodeResult::new(
            vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                normalize_final_content(content),
            ))],
            UsageDelta::default(),
        ))
    }
}

#[async_trait]
pub trait TurnWriterT: Send + Sync {
    async fn save_turn_atomic(
        &self,
        conversation_id: u64,
        user_id: u64,
        user: NewConversationMessage,
        assistant: NewConversationMessage,
    ) -> Result<PersistedTurn, AppError>;
}

pub struct ConversationTurnWriter {
    repository: Arc<dyn ConversationRepoT>,
}

impl ConversationTurnWriter {
    pub fn new(repository: Arc<dyn ConversationRepoT>) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl TurnWriterT for ConversationTurnWriter {
    async fn save_turn_atomic(
        &self,
        conversation_id: u64,
        user_id: u64,
        user: NewConversationMessage,
        assistant: NewConversationMessage,
    ) -> Result<PersistedTurn, AppError> {
        let (user, assistant) = self
            .repository
            .save_turn_atomic(conversation_id, user_id, user, assistant)
            .await?;
        Ok(PersistedTurn::new(user.id, assistant.id))
    }
}

pub struct PersistTurnNode {
    id: NodeId,
    writer: Arc<dyn TurnWriterT>,
}

impl PersistTurnNode {
    pub fn new(id: NodeId, writer: Arc<dyn TurnWriterT>) -> Self {
        Self { id, writer }
    }
}

#[async_trait]
impl AgentNode<ChatTurnState> for PersistTurnNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<ChatTurnState>,
        _context: &RunContext,
    ) -> Result<NodeResult<ChatTurnUpdate>, NodeError> {
        let turn = state.business();
        let reply = state
            .outcome()
            .and_then(AgentOutcome::response_text)
            .ok_or_else(|| {
                NodeError::new(NodeErrorKind::Invariant, "持久化前 AgentOutcome 尚未设置")
            })?;
        let user_content = serde_json::json!({
            "text": turn.user_message(),
            "emotion": turn.emotion(),
        });
        let assistant_content = serde_json::json!({ "text": reply });
        let conversation_id = turn.conversation_id();
        let user_id = turn.user_id();

        let persisted = self
            .writer
            .save_turn_atomic(
                conversation_id,
                user_id,
                NewConversationMessage {
                    conversation_id,
                    sender_role: "user".into(),
                    sender_user_id: Some(user_id),
                    message_type: "text".into(),
                    content: user_content.to_string(),
                    token_count: None,
                },
                NewConversationMessage {
                    conversation_id,
                    sender_role: "assistant".into(),
                    sender_user_id: None,
                    message_type: "text".into(),
                    content: assistant_content.to_string(),
                    token_count: None,
                },
            )
            .await
            .map_err(NodeError::from_application)?;

        Ok(NodeResult::new(
            vec![AgentUpdate::Business(ChatTurnUpdate::SetPersistedTurn(
                persisted,
            ))],
            UsageDelta::default(),
        ))
    }
}

fn prepare_recent_messages(
    mut messages: Vec<crate::domain::llm::ChatMessage>,
    user_message: &str,
    emotion: Option<&str>,
    max_context_messages: usize,
) -> Vec<crate::domain::llm::ChatMessage> {
    let content = match emotion {
        Some(emotion) if !emotion.trim().is_empty() => {
            format!("{user_message}\n\n[user emotion: {}]", emotion.trim())
        }
        _ => user_message.to_owned(),
    };

    if let Some(last_user_message) = messages
        .iter_mut()
        .rev()
        .find(|message| message.role == "user")
    {
        if last_user_message.content.trim() == user_message.trim() {
            last_user_message.content = content;
            return apply_context_limit(messages, max_context_messages);
        }
    }

    messages.push(crate::domain::llm::ChatMessage {
        role: "user".into(),
        content,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    });
    apply_context_limit(messages, max_context_messages)
}

fn apply_context_limit(
    messages: Vec<crate::domain::llm::ChatMessage>,
    limit: usize,
) -> Vec<crate::domain::llm::ChatMessage> {
    if limit == 0 || messages.is_empty() {
        return messages;
    }
    let system_messages: Vec<_> = messages
        .iter()
        .filter(|message| message.role == "system")
        .cloned()
        .collect();
    let mut other_messages: Vec<_> = messages
        .into_iter()
        .filter(|message| message.role != "system")
        .collect();
    if other_messages.len() > limit {
        let skip = other_messages.len().saturating_sub(limit);
        other_messages = other_messages.into_iter().skip(skip).collect();
    }
    let mut result = system_messages;
    result.extend(other_messages);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent::chat_state::ChatTurnState;
    use crate::app::agent::graph::{AgentNode, NodeId, RunBudget, RunContext, RunTrace};
    use crate::domain::agent::{AgentMessage, AgentOutcome, AgentState, AgentUpdate};
    use crate::domain::conversation::conversation_message::NewConversationMessage;
    use crate::domain::llm::ChatMessage;
    use crate::shared::error::AppError;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    #[derive(Debug)]
    struct RecordedTurn {
        conversation_id: u64,
        user_id: u64,
        user: NewConversationMessage,
        assistant: NewConversationMessage,
    }

    struct FakeTurnWriter {
        recorded: Mutex<Option<RecordedTurn>>,
    }

    struct FailingTurnWriter;

    #[async_trait]
    impl TurnWriterT for FakeTurnWriter {
        async fn save_turn_atomic(
            &self,
            conversation_id: u64,
            user_id: u64,
            user: NewConversationMessage,
            assistant: NewConversationMessage,
        ) -> Result<PersistedTurn, AppError> {
            *self.recorded.lock().unwrap() = Some(RecordedTurn {
                conversation_id,
                user_id,
                user,
                assistant,
            });
            Ok(PersistedTurn::new(101, 102))
        }
    }

    #[async_trait]
    impl TurnWriterT for FailingTurnWriter {
        async fn save_turn_atomic(
            &self,
            _conversation_id: u64,
            _user_id: u64,
            _user: NewConversationMessage,
            _assistant: NewConversationMessage,
        ) -> Result<PersistedTurn, AppError> {
            Err(AppError::Conflict("turn changed".into()))
        }
    }

    fn id(value: &str) -> NodeId {
        NodeId::try_from(value).unwrap()
    }

    fn run_context() -> RunContext {
        RunContext::new(
            RunBudget::for_test(8),
            CancellationToken::new(),
            RunTrace::default(),
        )
    }

    fn message(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    #[tokio::test]
    async fn chat_prepare_deduplicates_current_user_and_adds_emotion() {
        let state = AgentState::new(ChatTurnState::new(
            7,
            9,
            "hello".into(),
            Some(" calm ".into()),
            None,
            vec![message("user", "hello")],
        ));
        let node = PrepareTurnNode::new(id("prepare"), 10);

        let result = node.execute(&state, &run_context()).await.unwrap();
        let mut state = state;
        state.apply_updates(result.updates).unwrap();

        assert_eq!(state.business().recent_messages().len(), 1);
        assert_eq!(
            state.business().recent_messages()[0].content,
            "hello\n\n[user emotion: calm]"
        );
    }

    #[tokio::test]
    async fn chat_prepare_preserves_system_messages_and_limits_other_messages() {
        let state = AgentState::new(ChatTurnState::new(
            7,
            9,
            "latest".into(),
            None,
            None,
            vec![
                message("system", "system"),
                message("user", "old"),
                message("assistant", "reply"),
            ],
        ));
        let node = PrepareTurnNode::new(id("prepare"), 2);

        let result = node.execute(&state, &run_context()).await.unwrap();
        let mut state = state;
        state.apply_updates(result.updates).unwrap();

        let messages = state.business().recent_messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].content, "reply");
        assert_eq!(messages[2].content, "latest");
    }

    #[tokio::test]
    async fn chat_normalize_strips_artifacts_and_sets_outcome() {
        let mut state =
            AgentState::new(ChatTurnState::new(7, 9, "hello".into(), None, None, vec![]));
        state
            .apply_updates(vec![AgentUpdate::AppendMessages(vec![
                AgentMessage::assistant(
                    "<tool_call>{\"name\":\"clock\",\"arguments\":{}}</tool_call>现在 12 点。",
                    vec![],
                ),
            ])])
            .unwrap();
        let node = NormalizeReplyNode::new(id("normalize"));

        let result = node.execute(&state, &run_context()).await.unwrap();
        state.apply_updates(result.updates).unwrap();

        assert_eq!(
            state.outcome().and_then(AgentOutcome::response_text),
            Some("现在 12 点。")
        );
    }

    #[tokio::test]
    async fn chat_normalize_uses_fallback_without_assistant_content() {
        let mut state =
            AgentState::new(ChatTurnState::new(7, 9, "hello".into(), None, None, vec![]));
        let node = NormalizeReplyNode::new(id("normalize"));

        let result = node.execute(&state, &run_context()).await.unwrap();
        state.apply_updates(result.updates).unwrap();

        assert!(
            state
                .outcome()
                .and_then(AgentOutcome::response_text)
                .unwrap()
                .contains("抱歉")
        );
    }

    #[tokio::test]
    async fn chat_persist_uses_atomic_writer_with_compatible_json() {
        let writer = Arc::new(FakeTurnWriter {
            recorded: Mutex::new(None),
        });
        let mut state = AgentState::new(ChatTurnState::new(
            7,
            9,
            "hello".into(),
            Some("calm".into()),
            None,
            vec![],
        ));
        state
            .apply_updates(vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                "world".into(),
            ))])
            .unwrap();
        let node = PersistTurnNode::new(id("persist"), writer.clone());

        let result = node.execute(&state, &run_context()).await.unwrap();
        state.apply_updates(result.updates).unwrap();

        let recorded = writer.recorded.lock().unwrap();
        let recorded = recorded.as_ref().unwrap();
        assert_eq!(recorded.conversation_id, 9);
        assert_eq!(recorded.user_id, 7);
        assert_eq!(recorded.user.sender_role, "user");
        assert_eq!(recorded.user.sender_user_id, Some(7));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&recorded.user.content).unwrap(),
            serde_json::json!({"text": "hello", "emotion": "calm"})
        );
        assert_eq!(recorded.assistant.sender_role, "assistant");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&recorded.assistant.content).unwrap(),
            serde_json::json!({"text": "world"})
        );
        assert_eq!(
            state.business().persisted_turn().unwrap().user_message_id(),
            101
        );
    }

    #[tokio::test]
    async fn chat_persist_preserves_application_error_variant() {
        let node = PersistTurnNode::new(id("persist"), Arc::new(FailingTurnWriter));
        let mut state =
            AgentState::new(ChatTurnState::new(7, 9, "hello".into(), None, None, vec![]));
        state
            .apply_updates(vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                "done".into(),
            ))])
            .unwrap();

        let error = node.execute(&state, &run_context()).await.unwrap_err();

        assert!(matches!(
            error.application_error(),
            Some(AppError::Conflict(message)) if message == "turn changed"
        ));
    }
}
