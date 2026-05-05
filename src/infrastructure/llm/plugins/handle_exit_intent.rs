use async_trait::async_trait;
use serde_json::{Value, json};

use crate::domain::llm::tools::{LlmTool, ToolExecutionContext, ToolOutcome};

pub struct HandleExitIntentTool;

impl HandleExitIntentTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl LlmTool for HandleExitIntentTool {
    fn name(&self) -> &str {
        "handle_exit_intent"
    }

    fn tool_definition(&self) -> serde_json::Value {
        json!({
            "type": "function",
            "function": {
                "name": "handle_exit_intent",
                "description": "Call when the user wants to end the conversation.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "say_goodbye": {
                            "type": "string",
                            "description": "A friendly goodbye message for the user"
                        }
                    },
                    "required": ["say_goodbye"]
                }
            }
        })
    }

    async fn invoke(&self, _context: &mut ToolExecutionContext, arguments: &Value) -> ToolOutcome {
        let goodbye = arguments
            .get("say_goodbye")
            .and_then(|v| v.as_str())
            .unwrap_or("Okay, talk next time.")
            .to_string();
        ToolOutcome::reply_and_end(goodbye)
    }
}
