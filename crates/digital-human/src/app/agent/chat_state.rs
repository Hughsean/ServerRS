use crate::domain::agent::{
    AgentBusinessState, AgentContext, AgentStateError, ChatApprovalPreview,
    ChatApprovalPreviewSource, ChatApprovalToolCallPreview, CheckpointIdentity,
};
use crate::domain::llm::ChatMessage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::chat_effect::ChatEffect;
use super::reasoning_state::{ReasoningState, ToolApprovalStatus};

pub(crate) const CHECKPOINT_OWNER_MISMATCH: &str = "Checkpoint 不属于当前用户";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolApprovalRequest {
    pub approval_id: Uuid,
    pub prompt: String,
    pub tools: Vec<ApprovalToolCall>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ChatSuspendData {
    ToolApproval(ToolApprovalRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone)]
pub struct ChatResumeInput {
    pub user_id: u64,
    pub approval_id: Uuid,
    pub decision: ToolApprovalDecision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingToolApproval {
    request: ToolApprovalRequest,
    decision: Option<ToolApprovalDecision>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatResponseMode {
    #[default]
    Text,
    Audio,
}

/// HTTP Chat 图的业务扩展状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurnState {
    user_id: u64,
    conversation_id: u64,
    user_message: String,
    emotion: Option<String>,
    location: Option<Value>,
    recent_messages: Vec<ChatMessage>,
    #[serde(default)]
    response_mode: ChatResponseMode,
    messages_prepared: bool,
    context: Option<AgentContext>,
    context_version: Option<u64>,
    tool_depth: usize,
    persisted_turn: Option<PersistedTurn>,
    pending_tool_approval: Option<PendingToolApproval>,
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
        Self::with_response_mode(
            user_id,
            conversation_id,
            user_message,
            emotion,
            location,
            recent_messages,
            ChatResponseMode::Text,
        )
    }

    pub fn with_response_mode(
        user_id: u64,
        conversation_id: u64,
        user_message: String,
        emotion: Option<String>,
        location: Option<Value>,
        recent_messages: Vec<ChatMessage>,
        response_mode: ChatResponseMode,
    ) -> Self {
        Self {
            user_id,
            conversation_id,
            user_message,
            emotion,
            location,
            recent_messages,
            response_mode,
            messages_prepared: false,
            context: None,
            context_version: None,
            tool_depth: 0,
            persisted_turn: None,
            pending_tool_approval: None,
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

    pub fn response_mode(&self) -> ChatResponseMode {
        self.response_mode
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

    pub fn pending_tool_approval(&self) -> Option<&ToolApprovalRequest> {
        self.pending_tool_approval
            .as_ref()
            .map(|approval| &approval.request)
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
    SetPendingToolApproval(PendingToolApproval),
    ResolveToolApproval {
        user_id: u64,
        approval_id: Uuid,
        decision: ToolApprovalDecision,
    },
    ClearToolApproval,
}

impl AgentBusinessState for ChatTurnState {
    type Update = ChatTurnUpdate;
    type Effect = ChatEffect;
    type SuspendData = ChatSuspendData;
    type ResumeInput = ChatResumeInput;

    fn state_schema_version() -> crate::domain::agent::StateSchemaVersion {
        crate::domain::agent::StateSchemaVersion::try_from(2).expect("static schema version")
    }

    fn resume_updates(
        input: Self::ResumeInput,
    ) -> Vec<crate::domain::agent::AgentUpdate<Self::Update>> {
        vec![crate::domain::agent::AgentUpdate::Business(
            ChatTurnUpdate::ResolveToolApproval {
                user_id: input.user_id,
                approval_id: input.approval_id,
                decision: input.decision,
            },
        )]
    }

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
            ChatTurnUpdate::SetPendingToolApproval(approval) => {
                if self.pending_tool_approval.is_some() {
                    return Err(AgentStateError::Business(
                        "当前运行已经存在待处理的工具审批".into(),
                    ));
                }
                self.pending_tool_approval = Some(approval);
            }
            ChatTurnUpdate::ResolveToolApproval {
                user_id,
                approval_id,
                decision,
            } => {
                if user_id != self.user_id {
                    return Err(AgentStateError::Business(CHECKPOINT_OWNER_MISMATCH.into()));
                }
                let approval = self.pending_tool_approval.as_mut().ok_or_else(|| {
                    AgentStateError::Business("Checkpoint 不包含待处理的工具审批".into())
                })?;
                if approval.request.approval_id != approval_id {
                    return Err(AgentStateError::Business("审批标识不匹配".into()));
                }
                if approval.decision.is_some() {
                    return Err(AgentStateError::Business("工具审批已经处理".into()));
                }
                approval.decision = Some(decision);
            }
            ChatTurnUpdate::ClearToolApproval => {
                if self.pending_tool_approval.take().is_none() {
                    return Err(AgentStateError::Business("当前没有可清理的工具审批".into()));
                }
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

    fn request_tool_approval(
        &self,
        calls: &[crate::domain::agent::AgentToolCall],
    ) -> Option<(Self::Update, Self::SuspendData)> {
        let request = ToolApprovalRequest {
            approval_id: Uuid::new_v4(),
            prompt: "模型请求执行受控工具，请确认是否允许。".into(),
            tools: calls
                .iter()
                .map(|call| ApprovalToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                })
                .collect(),
        };
        let pending = PendingToolApproval {
            request: request.clone(),
            decision: None,
        };
        Some((
            ChatTurnUpdate::SetPendingToolApproval(pending),
            ChatSuspendData::ToolApproval(request),
        ))
    }

    fn tool_approval_status(&self) -> ToolApprovalStatus {
        match self
            .pending_tool_approval
            .as_ref()
            .and_then(|approval| approval.decision)
        {
            None if self.pending_tool_approval.is_some() => ToolApprovalStatus::Pending,
            None => ToolApprovalStatus::NotRequired,
            Some(ToolApprovalDecision::Approve) => ToolApprovalStatus::Approved,
            Some(ToolApprovalDecision::Reject) => ToolApprovalStatus::Rejected,
        }
    }

    fn clear_tool_approval_update() -> Option<Self::Update> {
        Some(ChatTurnUpdate::ClearToolApproval)
    }
}

impl CheckpointIdentity for ChatTurnState {
    fn checkpoint_user_id(&self) -> u64 {
        self.user_id()
    }

    fn checkpoint_conversation_id(&self) -> u64 {
        self.conversation_id()
    }
}

impl ChatApprovalPreviewSource for ChatSuspendData {
    /// 把暂停数据映射为可安全暴露给当前用户的审批预览。
    ///
    /// 预览只含审批 ID、提示文案和工具调用参数；不包含完整 Checkpoint
    /// payload、消息历史或内部 Trace。
    fn approval_preview(&self) -> Option<ChatApprovalPreview> {
        match self {
            ChatSuspendData::ToolApproval(request) => Some(ChatApprovalPreview {
                approval_id: request.approval_id,
                prompt: request.prompt.clone(),
                tool_calls: request
                    .tools
                    .iter()
                    .map(|tool| ChatApprovalToolCallPreview {
                        id: tool.id.clone(),
                        name: tool.name.clone(),
                        arguments: tool.arguments.clone(),
                    })
                    .collect(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    #[test]
    fn tool_approval_suspend_data_exposes_a_safe_preview() {
        let approval_id = Uuid::new_v4();
        let data = ChatSuspendData::ToolApproval(ToolApprovalRequest {
            approval_id,
            prompt: "模型请求执行受控工具，请确认是否允许。".into(),
            tools: vec![
                ApprovalToolCall {
                    id: "call-1".into(),
                    name: "fetch_web_content".into(),
                    arguments: serde_json::json!({"url": "https://example.com"}),
                },
                ApprovalToolCall {
                    id: "call-2".into(),
                    name: "get_time".into(),
                    arguments: serde_json::json!({}),
                },
            ],
        });

        let preview = data.approval_preview().expect("tool approval preview");

        assert_eq!(preview.approval_id, approval_id);
        assert_eq!(preview.prompt, "模型请求执行受控工具，请确认是否允许。");
        assert_eq!(preview.tool_calls.len(), 2);
        assert_eq!(preview.tool_calls[0].id, "call-1");
        assert_eq!(preview.tool_calls[0].name, "fetch_web_content");
        assert_eq!(
            preview.tool_calls[0].arguments,
            serde_json::json!({"url": "https://example.com"})
        );
        assert_eq!(preview.tool_calls[1].name, "get_time");
    }
}
