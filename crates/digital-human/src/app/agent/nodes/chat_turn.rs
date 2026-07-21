use crate::app::agent::chat_effect::{ChatEffect, PersistTurnEffect};
use crate::app::agent::chat_state::{ChatTurnState, ChatTurnUpdate};
use crate::app::agent::graph::{
    AgentNode, NodeError, NodeErrorKind, NodeId, NodeResult, RunContext, UsageDelta,
};
use crate::app::agent::memory_extraction::MemoryExtractionRequest;
use crate::app::agent::response::{fallback_reply, normalize_final_content};
use crate::domain::agent::{
    AgentBusinessState, AgentMessage, AgentOutcome, AgentState, AgentUpdate,
};
use crate::domain::conversation::conversation_message::NewConversationMessage;
use async_trait::async_trait;

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
    ) -> Result<NodeResult<ChatTurnUpdate, <ChatTurnState as AgentBusinessState>::Effect>, NodeError>
    {
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
    ) -> Result<NodeResult<ChatTurnUpdate, <ChatTurnState as AgentBusinessState>::Effect>, NodeError>
    {
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

pub struct PersistTurnNode {
    id: NodeId,
}

pub struct ScheduleMemoryExtractionNode {
    id: NodeId,
    enabled: bool,
}

impl ScheduleMemoryExtractionNode {
    pub fn new(id: NodeId, enabled: bool) -> Self {
        Self { id, enabled }
    }
}

#[async_trait]
impl AgentNode<ChatTurnState> for ScheduleMemoryExtractionNode {
    fn id(&self) -> &NodeId {
        &self.id
    }

    async fn execute(
        &self,
        state: &AgentState<ChatTurnState>,
        _context: &RunContext,
    ) -> Result<NodeResult<ChatTurnUpdate, <ChatTurnState as AgentBusinessState>::Effect>, NodeError>
    {
        if !self.enabled {
            return Ok(NodeResult::empty());
        }

        let turn = state.business();
        let persisted = turn.persisted_turn().ok_or_else(|| {
            NodeError::new(NodeErrorKind::Invariant, "调度记忆提取前本轮消息尚未持久化")
        })?;
        let context_version = turn.context_version().ok_or_else(|| {
            NodeError::new(NodeErrorKind::Invariant, "调度记忆提取前上下文版本尚未设置")
        })?;
        let assistant_reply = state
            .outcome()
            .and_then(AgentOutcome::response_text)
            .ok_or_else(|| {
                NodeError::new(
                    NodeErrorKind::Invariant,
                    "调度记忆提取前 AgentOutcome 尚未设置",
                )
            })?;

        Ok(NodeResult::with_effect(
            Vec::new(),
            ChatEffect::ScheduleMemoryExtraction(MemoryExtractionRequest {
                user_id: turn.user_id(),
                conversation_id: turn.conversation_id(),
                source_message_id: persisted.user_message_id(),
                user_message: turn.user_message().to_owned(),
                assistant_reply: assistant_reply.to_owned(),
                context_version,
            }),
            UsageDelta::default(),
        ))
    }
}

impl PersistTurnNode {
    pub fn new(id: NodeId) -> Self {
        Self { id }
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
    ) -> Result<NodeResult<ChatTurnUpdate, <ChatTurnState as AgentBusinessState>::Effect>, NodeError>
    {
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

        Ok(NodeResult::with_effect(
            Vec::new(),
            ChatEffect::PersistTurn(PersistTurnEffect {
                conversation_id,
                user_id,
                user: NewConversationMessage {
                    conversation_id,
                    sender_role: "user".into(),
                    sender_user_id: Some(user_id),
                    message_type: "text".into(),
                    content: user_content.to_string(),
                    token_count: None,
                },
                assistant: NewConversationMessage {
                    conversation_id,
                    sender_role: "assistant".into(),
                    sender_user_id: None,
                    message_type: "text".into(),
                    content: assistant_content.to_string(),
                    token_count: None,
                },
            }),
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
    use crate::app::agent::chat_effect::ChatEffect;
    use crate::app::agent::chat_state::{ChatTurnState, PersistedTurn};
    use crate::app::agent::graph::{AgentNode, NodeId, RunBudget, RunContext, RunTrace};
    use crate::domain::agent::{AgentContext, AgentMessage, AgentOutcome, AgentState, AgentUpdate};
    use crate::domain::llm::ChatMessage;
    use std::num::NonZeroU32;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn id(value: &str) -> NodeId {
        NodeId::try_from(value).unwrap()
    }

    fn run_context() -> RunContext {
        RunContext::new(
            RunBudget::new(NonZeroU32::new(8).unwrap(), Duration::from_secs(30)),
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
        state.apply_updates(result.into_updates()).unwrap();

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
        state.apply_updates(result.into_updates()).unwrap();

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
        state.apply_updates(result.into_updates()).unwrap();

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
        state.apply_updates(result.into_updates()).unwrap();

        assert!(
            state
                .outcome()
                .and_then(AgentOutcome::response_text)
                .unwrap()
                .contains("抱歉")
        );
    }

    #[tokio::test]
    async fn chat_persist_builds_compatible_effect_without_writer() {
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
        let node = PersistTurnNode::new(id("persist"));

        let result = node.execute(&state, &run_context()).await.unwrap();

        assert!(result.updates().is_empty());
        assert_eq!(result.effects().len(), 1);
        match &result.effects()[0] {
            ChatEffect::PersistTurn(effect) => {
                assert_eq!(effect.conversation_id, 9);
                assert_eq!(effect.user_id, 7);
                assert_eq!(effect.user.conversation_id, 9);
                assert_eq!(effect.user.sender_role, "user");
                assert_eq!(effect.user.sender_user_id, Some(7));
                assert_eq!(effect.user.message_type, "text");
                assert_eq!(effect.user.token_count, None);
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&effect.user.content).unwrap(),
                    serde_json::json!({"text": "hello", "emotion": "calm"})
                );
                assert_eq!(effect.assistant.conversation_id, 9);
                assert_eq!(effect.assistant.sender_role, "assistant");
                assert_eq!(effect.assistant.sender_user_id, None);
                assert_eq!(effect.assistant.message_type, "text");
                assert_eq!(effect.assistant.token_count, None);
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&effect.assistant.content).unwrap(),
                    serde_json::json!({"text": "world"})
                );
            }
            ChatEffect::ScheduleMemoryExtraction(_) => panic!("expected PersistTurn effect"),
        }
    }

    #[tokio::test]
    async fn memory_extraction_node_uses_persisted_turn_and_context_version() {
        let mut state =
            AgentState::new(ChatTurnState::new(7, 9, "hello".into(), None, None, vec![]));
        state
            .apply_updates(vec![
                AgentUpdate::Business(ChatTurnUpdate::SetContext {
                    context: AgentContext {
                        user_id: 7,
                        conversation_id: Some(9),
                        recent_messages: vec![],
                        summary: None,
                        memories: vec![],
                        rag_chunks: vec![],
                        fresh_chunks: vec![],
                        user_profile: None,
                        tools: vec![],
                        location: None,
                    },
                    context_version: 23,
                }),
                AgentUpdate::SetOutcome(AgentOutcome::Respond("world".into())),
                AgentUpdate::Business(ChatTurnUpdate::SetPersistedTurn(PersistedTurn::new(
                    101, 102,
                ))),
            ])
            .unwrap();
        let node = ScheduleMemoryExtractionNode::new(id("memory"), true);

        let result = node.execute(&state, &run_context()).await.unwrap();

        assert!(result.updates().is_empty());
        match &result.effects()[0] {
            ChatEffect::ScheduleMemoryExtraction(request) => {
                assert_eq!(request.user_id, 7);
                assert_eq!(request.conversation_id, 9);
                assert_eq!(request.source_message_id, 101);
                assert_eq!(request.user_message, "hello");
                assert_eq!(request.assistant_reply, "world");
                assert_eq!(request.context_version, 23);
            }
            ChatEffect::PersistTurn(_) => panic!("expected memory extraction effect"),
        }
    }

    #[tokio::test]
    async fn disabled_memory_extraction_node_is_a_noop() {
        let state = AgentState::new(ChatTurnState::new(7, 9, "hello".into(), None, None, vec![]));
        let node = ScheduleMemoryExtractionNode::new(id("memory"), false);

        let result = node.execute(&state, &run_context()).await.unwrap();

        assert!(result.updates().is_empty());
        assert!(result.effects().is_empty());
    }
}
