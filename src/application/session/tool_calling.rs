use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::domain::llm::tools::{LlmTool, ToolExecutionContext, ToolOutcome};
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
    ) -> ToolOutcome {
        match self.tools.get(name) {
            Some(tool) => tool.invoke(context, arguments).await,
            None => ToolOutcome::reply(format!("No tool found named {name}.")),
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
    /// Whether the tool outcome requests ending the session.
    pub end_session: bool,
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
                    end_session: false,
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
                        end_session: false,
                    };
                }
            };

            let message = match response.choices.first() {
                Some(choice) => &choice.message,
                None => {
                    return ToolCallResult {
                        reply: String::new(),
                        tool_invoked,
                        end_session: false,
                    };
                }
            };

            let tool_calls = match message.tool_calls.as_ref() {
                Some(value) => value,
                None => {
                    return ToolCallResult {
                        reply: message.content.clone().unwrap_or_default(),
                        tool_invoked,
                        end_session: false,
                    };
                }
            };

            let parsed_calls = parse_tool_calls(tool_calls);
            if parsed_calls.is_empty() {
                return ToolCallResult {
                    reply: message.content.clone().unwrap_or_default(),
                    tool_invoked,
                    end_session: false,
                };
            }

            tool_invoked = true;
            let mut assistant_tool_calls: Vec<Value> = Vec::new();
            let mut tool_outputs: Vec<ChatMessage> = Vec::new();

            for call in parsed_calls {
                debug!(tool = %call.name, "invoking tool");
                let outcome = self.invoke_tool(&call.name, context, &call.arguments).await;

                match outcome {
                    ToolOutcome::Reply { text, end_session } => {
                        return ToolCallResult {
                            reply: text,
                            tool_invoked: true,
                            end_session,
                        };
                    }
                    ToolOutcome::Continue(output) => {
                        assistant_tool_calls.push(call.raw);
                        tool_outputs.push(ChatMessage {
                            role: "tool".into(),
                            content: output,
                            tool_calls: None,
                            tool_call_id: Some(call.id),
                        });
                    }
                }
            }

            if assistant_tool_calls.is_empty() {
                return ToolCallResult {
                    reply: String::new(),
                    tool_invoked: true,
                    end_session: false,
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
    ) -> ToolOutcome {
        match timeout(
            self.tool_timeout,
            self.registry.invoke(name, context, arguments),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => ToolOutcome::reply(
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
