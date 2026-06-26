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
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub user_id: u64,
    pub conversation_id: Option<u64>,
    pub recent_messages: Vec<crate::domain::llm::ChatMessage>,
    pub summary: Option<String>,
    pub memories: Vec<String>,
    pub rag_chunks: Vec<String>,
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

// ── Agent action ──

/// 代理处理完一轮后产生的决策。
#[derive(Debug, Clone)]
pub enum AgentAction {
    /// Produce a final textual response to the user.
    Respond(String),
    /// Invoke the named tool with the given arguments.
    UseTool(String, Value),
    /// Escalate to a human safety reviewer with the provided reason.
    SafetyEscalate(String),
    /// Ask the user for clarification on an ambiguous request.
    Clarify(String),
}

// ── Agent policy ──

/// 控制代理行为的当前策略状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPolicy {
    /// Normal operation — agent executes freely.
    Normal,
    /// A safety filter blocked the last action; proceed with caution.
    SafetyBlocked,
    /// The agent is in a cooldown period (e.g. rate-limited or post-safety).
    Cooldown,
    /// Maximum recursion depth reached; force an early response.
    MaxDepthReached,
}

// ── Repository ──

/// 持久化代理事件的端口。
#[async_trait]
pub trait AgentEventRepoT: Send + Sync {
    /// 持久化新代理事件并返回完整填充的记录。
    async fn log_event(&self, event: NewAgentEvent) -> AgentEvent;
}
