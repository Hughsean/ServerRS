use async_trait::async_trait;
use chrono::Local;
use serde_json::{Value, json};

use crate::domain::llm::tools::{LlmTool, ToolExecutionContext, ToolResponse};

pub struct GetTimeTool;

impl GetTimeTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl LlmTool for GetTimeTool {
    fn name(&self) -> &str {
        "get_time"
    }

    fn tool_definition(&self) -> serde_json::Value {
        json!({
            "type": "function",
            "function": {
                "name": "get_time",
                "description": "Use when the user asks for the current time or date.",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        })
    }

    async fn invoke(
        &self,
        _context: &mut ToolExecutionContext,
        _arguments: &Value,
    ) -> ToolResponse {
        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        ToolResponse::requeue(format!("Current date/time is: {now}"))
    }
}
