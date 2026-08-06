use crate::AgentToolCall;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A provider-neutral action selected during an Agent run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentAction {
    Respond(String),
    UseTool(String, Value),
    CallTool(AgentToolCall),
    SafetyEscalate(String),
    Clarify(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentPolicy {
    Normal,
    SafetyBlocked,
    Cooldown,
    MaxDepthReached,
}
