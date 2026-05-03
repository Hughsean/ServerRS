use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::domain::llm::tools::{LlmTool, ToolAction, ToolExecutionContext, ToolResponse};
use crate::domain::llm::{ChatMessage, LlmClient};

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn LlmTool>>,
}

impl ToolRegistry {
    /// Create a registry from any iterable of tools.
    pub fn new(tools: impl IntoIterator<Item = Arc<dyn LlmTool>>) -> Self {
        let mut map = HashMap::new();
        for tool in tools {
            map.insert(tool.name().to_string(), tool);
        }
        Self { tools: map }
    }

    /// Register an additional tool post-construction (builder style).
    pub fn register(mut self, tool: Arc<dyn LlmTool>) -> Self {
        self.tools.insert(tool.name().to_string(), tool);
        self
    }

    pub fn tool_definitions(&self) -> Vec<Value> {
        self.tools.values().map(|t| t.tool_definition()).collect()
    }

    pub async fn invoke(
        &self,
        name: &str,
        context: &mut ToolExecutionContext,
        arguments: &Value,
    ) -> ToolResponse {
        match self.tools.get(name) {
            Some(tool) => tool.invoke(context, arguments).await,
            None => ToolResponse::respond(format!("No tool found named {name}.")),
        }
    }
}

pub struct ToolCallService {
    llm: Arc<dyn LlmClient>,
    registry: ToolRegistry,
    max_depth: usize,
    tool_timeout: Duration,
}

pub struct ToolCallResult {
    pub reply: String,
    pub tool_invoked: bool,
    pub exit_requested: bool,
}

impl ToolCallService {
    pub fn new(
        llm: Arc<dyn LlmClient>,
        registry: ToolRegistry,
        max_depth: usize,
        tool_timeout: Duration,
    ) -> Self {
        Self {
            llm,
            registry,
            max_depth,
            tool_timeout,
        }
    }

    pub async fn chat_with_tools(
        &self,
        messages: &mut Vec<ChatMessage>,
        context: &mut ToolExecutionContext,
    ) -> ToolCallResult {
        let tool_defs = self.registry.tool_definitions();
        let tools = if tool_defs.is_empty() {
            None
        } else {
            Some(tool_defs.as_slice())
        };
        let mut depth = 0usize;
        let mut tool_invoked = false;

        loop {
            if depth > self.max_depth {
                return ToolCallResult {
                    reply: "Tool call depth limit exceeded.".to_string(),
                    tool_invoked: true,
                    exit_requested: context.is_exit_requested(),
                };
            }

            let response = match self.llm.chat_raw(messages, tools).await {
                Ok(resp) => resp,
                Err(err) => {
                    warn!(error = %err, "tool call chat_raw failed; fallback to chat()");
                    let reply = self.llm.chat(messages).await;
                    return ToolCallResult {
                        reply,
                        tool_invoked,
                        exit_requested: context.is_exit_requested(),
                    };
                }
            };

            let message = match response.choices.first() {
                Some(choice) => &choice.message,
                None => {
                    return ToolCallResult {
                        reply: String::new(),
                        tool_invoked,
                        exit_requested: context.is_exit_requested(),
                    };
                }
            };

            let tool_calls = match message.tool_calls.as_ref() {
                Some(value) => value,
                None => {
                    return ToolCallResult {
                        reply: message.content.clone().unwrap_or_default(),
                        tool_invoked,
                        exit_requested: context.is_exit_requested(),
                    };
                }
            };

            let parsed_calls = parse_tool_calls(tool_calls);
            if parsed_calls.is_empty() {
                return ToolCallResult {
                    reply: message.content.clone().unwrap_or_default(),
                    tool_invoked,
                    exit_requested: context.is_exit_requested(),
                };
            }

            tool_invoked = true;
            let mut assistant_tool_calls: Vec<Value> = Vec::new();
            let mut tool_outputs: Vec<ChatMessage> = Vec::new();

            for call in parsed_calls {
                debug!(tool = %call.name, "invoking tool");
                let response = self.invoke_tool(&call.name, context, &call.arguments).await;

                match response.action {
                    ToolAction::Response => {
                        return ToolCallResult {
                            reply: response.response.unwrap_or_default(),
                            tool_invoked: true,
                            exit_requested: context.is_exit_requested(),
                        };
                    }
                    ToolAction::Requeue => {
                        assistant_tool_calls.push(call.raw);
                        tool_outputs.push(ChatMessage {
                            role: "tool".into(),
                            content: response.result.unwrap_or_default(),
                            tool_calls: None,
                            tool_call_id: Some(call.id),
                        });
                    }
                    ToolAction::None => {}
                }
            }

            if assistant_tool_calls.is_empty() {
                return ToolCallResult {
                    reply: String::new(),
                    tool_invoked: true,
                    exit_requested: context.is_exit_requested(),
                };
            }

            messages.push(ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                tool_calls: Some(Value::Array(assistant_tool_calls)),
                tool_call_id: None,
            });
            messages.extend(tool_outputs);
            depth += 1;
        }
    }

    async fn invoke_tool(
        &self,
        name: &str,
        context: &mut ToolExecutionContext,
        arguments: &Value,
    ) -> ToolResponse {
        match timeout(
            self.tool_timeout,
            self.registry.invoke(name, context, arguments),
        )
        .await
        {
            Ok(resp) => resp,
            Err(_) => ToolResponse::respond(
                "Sorry, the tool timed out. Please try again later or simplify the request.",
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct ParsedToolCall {
    id: String,
    name: String,
    arguments: Value,
    raw: Value,
}

fn parse_tool_calls(value: &Value) -> Vec<ParsedToolCall> {
    let calls = match value.as_array() {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    calls
        .iter()
        .filter_map(|call| {
            let id = call.get("id").and_then(|v| v.as_str())?.to_string();
            let function = call.get("function")?;
            let name = function.get("name").and_then(|v| v.as_str())?.to_string();
            let arguments = match function.get("arguments") {
                Some(Value::String(raw)) => {
                    serde_json::from_str(raw).unwrap_or(Value::Object(Default::default()))
                }
                Some(Value::Object(map)) => Value::Object(map.clone()),
                _ => Value::Object(Default::default()),
            };
            Some(ParsedToolCall {
                id,
                name,
                arguments,
                raw: call.clone(),
            })
        })
        .collect()
}
