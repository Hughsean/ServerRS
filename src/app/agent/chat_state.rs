use crate::domain::agent::{AgentBusinessState, AgentContext, AgentStateError};
use crate::domain::llm::ChatMessage;
use serde_json::Value;

use super::chat_effect::ChatEffect;
use super::reasoning_state::ReasoningState;

/// HTTP Chat 图的业务扩展状态。
#[derive(Debug, Clone)]
pub struct ChatTurnState {
    user_id: u64,
    conversation_id: u64,
    user_message: String,
    emotion: Option<String>,
    location: Option<Value>,
    recent_messages: Vec<ChatMessage>,
    messages_prepared: bool,
    context: Option<AgentContext>,
    context_version: Option<u64>,
    tool_depth: usize,
    persisted_turn: Option<PersistedTurn>,
}

impl ChatTurnState {
    pub fn new(
        user_id: u64,
        conversation_id: u64,
        user_message: String,
        emotion: Option<String>,
        location: Option<Value>,
        recent_messages: Vec<ChatMessage>,
    ) -> Self {
        Self {
            user_id,
            conversation_id,
            user_message,
            emotion,
            location,
            recent_messages,
            messages_prepared: false,
            context: None,
            context_version: None,
            tool_depth: 0,
            persisted_turn: None,
        }
    }

    pub fn user_id(&self) -> u64 {
        self.user_id
    }

    pub fn conversation_id(&self) -> u64 {
        self.conversation_id
    }

    pub fn user_message(&self) -> &str {
        &self.user_message
    }

    pub fn emotion(&self) -> Option<&str> {
        self.emotion.as_deref()
    }

    pub fn location(&self) -> Option<&Value> {
        self.location.as_ref()
    }

    pub fn recent_messages(&self) -> &[ChatMessage] {
        &self.recent_messages
    }

    pub fn messages_prepared(&self) -> bool {
        self.messages_prepared
    }

    pub fn context(&self) -> Option<&AgentContext> {
        self.context.as_ref()
    }

    pub fn context_version(&self) -> Option<u64> {
        self.context_version
    }

    pub fn tool_depth(&self) -> usize {
        self.tool_depth
    }

    pub fn persisted_turn(&self) -> Option<&PersistedTurn> {
        self.persisted_turn.as_ref()
    }
}

#[derive(Debug)]
pub enum ChatTurnUpdate {
    SetRecentMessages(Vec<ChatMessage>),
    SetContext {
        context: AgentContext,
        context_version: u64,
    },
    IncrementToolDepth,
    SetPersistedTurn(PersistedTurn),
}

impl AgentBusinessState for ChatTurnState {
    type Update = ChatTurnUpdate;
    type Effect = ChatEffect;

    fn apply_update(&mut self, update: Self::Update) -> Result<(), AgentStateError> {
        match update {
            ChatTurnUpdate::SetRecentMessages(messages) => {
                if self.messages_prepared {
                    return Err(AgentStateError::Business("本轮消息已经完成预处理".into()));
                }
                self.recent_messages = messages;
                self.messages_prepared = true;
            }
            ChatTurnUpdate::SetContext {
                context,
                context_version,
            } => {
                if self.context.is_some() {
                    return Err(AgentStateError::Business(
                        "本轮 AgentContext 已经设置".into(),
                    ));
                }
                if context.user_id != self.user_id
                    || context.conversation_id != Some(self.conversation_id)
                {
                    return Err(AgentStateError::Business(
                        "AgentContext 与当前 ChatTurn 不匹配".into(),
                    ));
                }
                self.context = Some(context);
                self.context_version = Some(context_version);
            }
            ChatTurnUpdate::IncrementToolDepth => {
                self.tool_depth = self
                    .tool_depth
                    .checked_add(1)
                    .ok_or_else(|| AgentStateError::Business("工具调用深度溢出".into()))?;
            }
            ChatTurnUpdate::SetPersistedTurn(persisted) => {
                if self.persisted_turn.is_some() {
                    return Err(AgentStateError::Business("本轮消息已经持久化".into()));
                }
                self.persisted_turn = Some(persisted);
            }
        }
        Ok(())
    }
}

impl ReasoningState for ChatTurnState {
    fn reasoning_context(&self) -> Option<&AgentContext> {
        self.context()
    }

    fn reasoning_tool_depth(&self) -> usize {
        self.tool_depth()
    }

    fn reasoning_user_id(&self) -> u64 {
        self.user_id()
    }

    fn reasoning_conversation_id(&self) -> Option<u64> {
        Some(self.conversation_id())
    }

    fn increment_reasoning_tool_depth() -> Self::Update {
        ChatTurnUpdate::IncrementToolDepth
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedTurn {
    user_message_id: u64,
    assistant_message_id: u64,
}

impl PersistedTurn {
    pub fn new(user_message_id: u64, assistant_message_id: u64) -> Self {
        Self {
            user_message_id,
            assistant_message_id,
        }
    }

    pub fn user_message_id(&self) -> u64 {
        self.user_message_id
    }

    pub fn assistant_message_id(&self) -> u64 {
        self.assistant_message_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent::{AgentBusinessState, AgentContext};

    #[test]
    fn chat_turn_update_increments_tool_depth() {
        let mut state = ChatTurnState::new(7, 9, "hello".into(), None, None, vec![]);

        state
            .apply_update(ChatTurnUpdate::IncrementToolDepth)
            .unwrap();

        assert_eq!(state.tool_depth(), 1);
    }

    #[test]
    fn context_and_version_are_set_together() {
        let mut state = ChatTurnState::new(7, 9, "hello".into(), None, None, vec![]);
        let context = AgentContext {
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
        };

        state
            .apply_update(ChatTurnUpdate::SetContext {
                context,
                context_version: 13,
            })
            .unwrap();

        assert_eq!(state.context().unwrap().user_id, 7);
        assert_eq!(state.context_version(), Some(13));
    }

    #[test]
    fn persistence_ids_are_recorded_as_one_update() {
        let mut state = ChatTurnState::new(7, 9, "hello".into(), None, None, vec![]);

        state
            .apply_update(ChatTurnUpdate::SetPersistedTurn(PersistedTurn::new(
                101, 102,
            )))
            .unwrap();

        let persisted = state.persisted_turn().unwrap();
        assert_eq!(persisted.user_message_id(), 101);
        assert_eq!(persisted.assistant_message_id(), 102);
    }

    #[test]
    fn prepared_messages_replace_the_compatibility_history() {
        let mut state = ChatTurnState::new(
            7,
            9,
            "hello".into(),
            None,
            None,
            vec![crate::domain::llm::ChatMessage {
                role: "user".into(),
                content: "old".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }],
        );
        let prepared = vec![crate::domain::llm::ChatMessage {
            role: "user".into(),
            content: "hello".into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];

        state
            .apply_update(ChatTurnUpdate::SetRecentMessages(prepared))
            .unwrap();

        assert_eq!(state.recent_messages()[0].content, "hello");
    }
}
