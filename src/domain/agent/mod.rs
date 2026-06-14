use crate::domain::llm::tools::LlmTool;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Agent event ──

/// A structured log entry recording an agent's internal action.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentEvent {
    pub event_id: u64,
    pub user_id: u64,
    pub conversation_id: Option<u64>,
    pub trace_id: Option<String>,
    /// One of: plan, tool_call, tool_result, rag_retrieval, memory_write, safety_block.
    pub event_type: String,
    pub tool_name: Option<String>,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

/// Input used when persisting a new agent event (without a pre-assigned id or timestamp).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NewAgentEvent {
    pub user_id: u64,
    pub conversation_id: Option<u64>,
    pub event_type: String,
    pub tool_name: Option<String>,
    pub payload: Value,
}

// ── Agent context ──

/// All contextual information the agent needs to process a single turn.
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

/// Description of a tool as presented to the agent (and to the LLM).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub parameters: Value,
}

impl ToolDefinition {
    /// Build a definition from an `LlmTool` implementor.
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

/// The decision an agent produces after processing a turn.
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

/// The current policy state governing agent behaviour.
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

/// Port for persisting agent events.
#[async_trait]
pub trait AgentEventRepository: Send + Sync {
    /// Persist a new agent event and return the fully populated record.
    async fn log_event(&self, event: NewAgentEvent) -> AgentEvent;
}
