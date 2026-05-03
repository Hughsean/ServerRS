use async_trait::async_trait;
use serde_json::{Value, json};

use crate::domain::llm::tools::{LlmTool, ToolExecutionContext, ToolResponse};

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

    async fn invoke(&self, context: &mut ToolExecutionContext, arguments: &Value) -> ToolResponse {
        let goodbye = arguments
            .get("say_goodbye")
            .and_then(|v| v.as_str())
            .unwrap_or("Okay, talk next time.")
            .to_string();
        context.mark_exit_requested();
        ToolResponse::respond(goodbye)
    }
}
