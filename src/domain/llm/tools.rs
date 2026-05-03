use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAction {
    None,
    Response,
    Requeue,
}

#[derive(Debug, Clone)]
pub struct ToolResponse {
    pub action: ToolAction,
    pub response: Option<String>,
    pub result: Option<String>,
}

impl ToolResponse {
    pub fn none() -> Self {
        Self {
            action: ToolAction::None,
            response: None,
            result: None,
        }
    }

    pub fn respond(message: impl Into<String>) -> Self {
        Self {
            action: ToolAction::Response,
            response: Some(message.into()),
            result: None,
        }
    }

    pub fn requeue(payload: impl Into<String>) -> Self {
        Self {
            action: ToolAction::Requeue,
            response: None,
            result: Some(payload.into()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolExecutionContext {
    attributes: HashMap<String, Value>,
    exit_requested: bool,
}

impl ToolExecutionContext {
    pub fn is_exit_requested(&self) -> bool {
        self.exit_requested
    }

    pub fn mark_exit_requested(&mut self) {
        self.exit_requested = true;
    }

    pub fn set_attribute(&mut self, key: impl Into<String>, value: Value) {
        self.attributes.insert(key.into(), value);
    }

    pub fn get_attribute(&self, key: &str) -> Option<&Value> {
        self.attributes.get(key)
    }

    pub fn attributes_snapshot(&self) -> HashMap<String, Value> {
        self.attributes.clone()
    }
}

#[async_trait]
pub trait LlmTool: Send + Sync {
    fn name(&self) -> &str;
    fn tool_definition(&self) -> Value;
    async fn invoke(&self, context: &mut ToolExecutionContext, arguments: &Value) -> ToolResponse;
}
