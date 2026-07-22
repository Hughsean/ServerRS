mod approval_inbox;

pub use agent_core::{
    AgentAction, AgentBusinessState, AgentMessage, AgentObservation, AgentOutcome, AgentPolicy,
    AgentState, AgentStateError, AgentToolCall, AgentUpdate, PromptSection, PromptSource,
    PromptTrust, StateSchemaVersion,
};
pub use approval_inbox::{
    CHAT_APPROVAL_DECISION_EVENT, ChatApprovalAuditT, ChatApprovalDecision,
    ChatApprovalDecisionEvent, ChatApprovalPreview, ChatApprovalPreviewSource, ChatApprovalQueryT,
    ChatApprovalToolCallPreview, PENDING_APPROVAL_DEFAULT_LIMIT, PENDING_APPROVAL_MAX_LIMIT,
    PendingApprovalPage, PendingChatApproval,
};

use crate::domain::llm::tools::LlmTool;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Agent event ──

/// 记录代理内部操作的结构化日志条目。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentEvent {
    pub event_id: u64,
    pub user_id: u64,
    pub conversation_id: Option<u64>,
    pub trace_id: Option<String>,
    /// 取值之一：plan, tool_call, tool_result, rag_retrieval, memory_write, safety_block。
    pub event_type: String,
    pub tool_name: Option<String>,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

/// 持久化新代理事件时使用的输入（没有预分配的 id 或时间戳）。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NewAgentEvent {
    pub user_id: u64,
    pub conversation_id: Option<u64>,
    pub event_type: String,
    pub tool_name: Option<String>,
    pub payload: Value,
}

// ── Agent context ──

/// 代理处理单轮所需的所有上下文信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContext {
    pub user_id: u64,
    pub conversation_id: Option<u64>,
    pub recent_messages: Vec<crate::domain::llm::ChatMessage>,
    pub summary: Option<String>,
    pub memories: Vec<String>,
    pub rag_chunks: Vec<String>,
    pub fresh_chunks: Vec<String>,
    pub user_profile: Option<Value>,
    pub tools: Vec<ToolDefinition>,
    pub location: Option<Value>,
}

/// 呈现给代理（和 LLM）的工具描述。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// 工具输入参数的 JSON Schema。
    pub parameters: Value,
}

impl ToolDefinition {
    /// 从 `LlmTool` 实现者构建定义。
    pub fn from_tool(tool: &dyn LlmTool) -> Self {
        let def = tool.tool_definition();
        Self {
            name: def
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            description: def
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            parameters: def.get("parameters").cloned().unwrap_or(Value::Null),
        }
    }
}

// ── Repository ──

/// 持久化代理事件的端口。
#[async_trait]
pub trait AgentEventRepoT: Send + Sync {
    /// 持久化新代理事件并返回完整填充的记录。
    async fn log_event(&self, event: NewAgentEvent) -> AgentEvent;
}

/// 持久化 Checkpoint 的最小归属元数据。
///
/// 基础设施适配器只依赖这个领域接口，不需要了解具体应用状态。
pub trait CheckpointIdentity {
    fn checkpoint_user_id(&self) -> u64;
    fn checkpoint_conversation_id(&self) -> u64;
}

#[cfg(test)]
mod state_model_tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestBusiness(u32);

    enum TestUpdate {
        Set(u32),
        Reject,
    }

    impl AgentBusinessState for TestBusiness {
        type Update = TestUpdate;
        type Effect = ();
        type SuspendData = ();
        type ResumeInput = ();

        fn resume_updates(_input: Self::ResumeInput) -> Vec<AgentUpdate<Self::Update>> {
            Vec::new()
        }

        fn apply_update(&mut self, update: Self::Update) -> Result<(), AgentStateError> {
            match update {
                TestUpdate::Set(value) => {
                    self.0 = value;
                    Ok(())
                }
                TestUpdate::Reject => Err(AgentStateError::Business("rejected".into())),
            }
        }
    }

    #[test]
    fn update_batch_is_atomic_when_business_update_fails() {
        let mut state = AgentState::new(TestBusiness(1));

        let result = state.apply_updates(vec![
            AgentUpdate::AppendMessages(vec![AgentMessage::user("hello")]),
            AgentUpdate::Business(TestUpdate::Reject),
        ]);

        assert!(result.is_err());
        assert!(state.messages().is_empty());
        assert_eq!(state.business(), &TestBusiness(1));
    }

    #[test]
    fn outcome_is_write_once() {
        let mut state = AgentState::new(TestBusiness(1));
        state
            .apply_updates(vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                "first".into(),
            ))])
            .unwrap();

        let result = state.apply_updates(vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
            "second".into(),
        ))]);

        assert!(matches!(result, Err(AgentStateError::TerminalState)));
        assert_eq!(
            state.outcome().and_then(AgentOutcome::response_text),
            Some("first")
        );
    }

    #[test]
    fn terminal_state_allows_only_business_updates() {
        let mut state = AgentState::new(TestBusiness(1));
        state
            .apply_updates(vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
                "done".into(),
            ))])
            .unwrap();

        assert!(matches!(
            state.apply_updates(vec![AgentUpdate::AppendMessages(vec![AgentMessage::user(
                "late",
            )])]),
            Err(AgentStateError::TerminalState)
        ));

        state
            .apply_updates(vec![AgentUpdate::Business(TestUpdate::Set(2))])
            .unwrap();
        assert_eq!(state.business(), &TestBusiness(2));
    }

    #[test]
    fn pending_actions_are_replaced_explicitly() {
        let mut state = AgentState::new(TestBusiness(1));
        let action = AgentAction::CallTool(AgentToolCall {
            id: "call-1".into(),
            name: "get_time".into(),
            arguments: serde_json::json!({}),
        });

        state
            .apply_updates(vec![AgentUpdate::ReplacePendingActions(vec![action])])
            .unwrap();
        assert_eq!(state.pending_actions().len(), 1);

        state
            .apply_updates(vec![AgentUpdate::ReplacePendingActions(Vec::new())])
            .unwrap();
        assert!(state.pending_actions().is_empty());
    }
}
