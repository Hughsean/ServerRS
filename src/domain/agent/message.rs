use crate::domain::llm::ChatMessage;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 与具体 LLM Provider 解耦的 Agent 消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: String,
        tool_calls: Vec<AgentToolCall>,
    },
    Tool {
        call_id: String,
        name: String,
        content: String,
    },
}

impl AgentMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>, tool_calls: Vec<AgentToolCall>) -> Self {
        Self::Assistant {
            content: content.into(),
            tool_calls,
        }
    }

    pub fn tool(
        call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self::Tool {
            call_id: call_id.into(),
            name: name.into(),
            content: content.into(),
        }
    }

    pub fn content(&self) -> &str {
        match self {
            Self::System { content }
            | Self::User { content }
            | Self::Assistant { content, .. }
            | Self::Tool { content, .. } => content,
        }
    }

    pub fn tool_calls(&self) -> &[AgentToolCall] {
        match self {
            Self::Assistant { tool_calls, .. } => tool_calls,
            _ => &[],
        }
    }
}

/// 一次类型化工具调用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// 工具执行后写回状态的观察结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentObservation {
    pub call: AgentToolCall,
    pub result: String,
    pub succeeded: bool,
}

/// Agent 一轮运行的业务结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentOutcome {
    Respond(String),
}

impl AgentOutcome {
    pub fn response_text(&self) -> Option<&str> {
        match self {
            Self::Respond(text) => Some(text),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AgentMessageError {
    #[error("不支持的 Agent 消息角色: {0}")]
    UnsupportedRole(String),
    #[error("工具调用格式无效: {0}")]
    InvalidToolCalls(String),
    #[error("工具消息缺少字段: {0}")]
    MissingToolField(&'static str),
}

impl TryFrom<ChatMessage> for AgentMessage {
    type Error = AgentMessageError;

    fn try_from(message: ChatMessage) -> Result<Self, Self::Error> {
        let ChatMessage {
            role,
            content,
            tool_calls,
            tool_call_id,
            name,
        } = message;

        match role.as_str() {
            "system" => Ok(Self::system(content)),
            "user" => Ok(Self::user(content)),
            "assistant" => Ok(Self::assistant(content, parse_tool_calls(tool_calls)?)),
            "tool" => Ok(Self::tool(
                tool_call_id.ok_or(AgentMessageError::MissingToolField("tool_call_id"))?,
                name.ok_or(AgentMessageError::MissingToolField("name"))?,
                content,
            )),
            _ => Err(AgentMessageError::UnsupportedRole(role)),
        }
    }
}

impl From<AgentMessage> for ChatMessage {
    fn from(message: AgentMessage) -> Self {
        match message {
            AgentMessage::System { content } => Self {
                role: "system".into(),
                content,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            AgentMessage::User { content } => Self {
                role: "user".into(),
                content,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            AgentMessage::Assistant {
                content,
                tool_calls,
            } => Self {
                role: "assistant".into(),
                content,
                tool_calls: (!tool_calls.is_empty()).then(|| {
                    Value::Array(
                        tool_calls
                            .into_iter()
                            .map(|call| {
                                let arguments = match call.arguments {
                                    Value::String(arguments) => arguments,
                                    other => other.to_string(),
                                };
                                json!({
                                    "id": call.id,
                                    "type": "function",
                                    "function": {
                                        "name": call.name,
                                        "arguments": arguments,
                                    }
                                })
                            })
                            .collect(),
                    )
                }),
                tool_call_id: None,
                name: None,
            },
            AgentMessage::Tool {
                call_id,
                name,
                content,
            } => Self {
                role: "tool".into(),
                content,
                tool_calls: None,
                tool_call_id: Some(call_id),
                name: Some(name),
            },
        }
    }
}

fn parse_tool_calls(tool_calls: Option<Value>) -> Result<Vec<AgentToolCall>, AgentMessageError> {
    let Some(tool_calls) = tool_calls else {
        return Ok(Vec::new());
    };
    if tool_calls.is_null() {
        return Ok(Vec::new());
    }

    let calls = tool_calls
        .as_array()
        .ok_or_else(|| AgentMessageError::InvalidToolCalls("tool_calls 必须是数组".into()))?;

    calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let id = call.get("id").and_then(Value::as_str).ok_or_else(|| {
                AgentMessageError::InvalidToolCalls(format!("第 {index} 项缺少 id"))
            })?;
            let function = call.get("function").ok_or_else(|| {
                AgentMessageError::InvalidToolCalls(format!("第 {index} 项缺少 function"))
            })?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AgentMessageError::InvalidToolCalls(format!("第 {index} 项缺少 function.name"))
                })?;
            let arguments = function.get("arguments").cloned().ok_or_else(|| {
                AgentMessageError::InvalidToolCalls(format!("第 {index} 项缺少 function.arguments"))
            })?;

            Ok(AgentToolCall {
                id: id.to_owned(),
                name: name.to_owned(),
                arguments,
            })
        })
        .collect()
}
