use crate::domain::agent::{AgentMessage, AgentToolCall};
use crate::domain::llm::ChatMessage;
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum AgentMessageConversionError {
    #[error("不支持的 Agent 消息角色: {0}")]
    UnsupportedRole(String),
    #[error("工具调用格式无效: {0}")]
    InvalidToolCalls(String),
    #[error("工具消息缺少字段: {0}")]
    MissingToolField(&'static str),
}

pub(crate) fn agent_message_from_chat(
    message: ChatMessage,
) -> Result<AgentMessage, AgentMessageConversionError> {
    let ChatMessage {
        role,
        content,
        tool_calls,
        tool_call_id,
        name,
    } = message;

    match role.as_str() {
        "system" => Ok(AgentMessage::system(content)),
        "user" => Ok(AgentMessage::user(content)),
        "assistant" => Ok(AgentMessage::assistant(
            content,
            parse_tool_calls(tool_calls)?,
        )),
        "tool" => Ok(AgentMessage::tool(
            tool_call_id.ok_or(AgentMessageConversionError::MissingToolField(
                "tool_call_id",
            ))?,
            name.ok_or(AgentMessageConversionError::MissingToolField("name"))?,
            content,
        )),
        _ => Err(AgentMessageConversionError::UnsupportedRole(role)),
    }
}

pub(crate) fn chat_message_from_agent(message: AgentMessage) -> ChatMessage {
    match message {
        AgentMessage::System { content } => ChatMessage {
            role: "system".into(),
            content,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        },
        AgentMessage::User { content } => ChatMessage {
            role: "user".into(),
            content,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        },
        AgentMessage::Assistant {
            content,
            tool_calls,
        } => ChatMessage {
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
        } => ChatMessage {
            role: "tool".into(),
            content,
            tool_calls: None,
            tool_call_id: Some(call_id),
            name: Some(name),
        },
    }
}

fn parse_tool_calls(
    tool_calls: Option<Value>,
) -> Result<Vec<AgentToolCall>, AgentMessageConversionError> {
    let Some(tool_calls) = tool_calls else {
        return Ok(Vec::new());
    };
    if tool_calls.is_null() {
        return Ok(Vec::new());
    }

    let calls = tool_calls.as_array().ok_or_else(|| {
        AgentMessageConversionError::InvalidToolCalls("tool_calls 必须是数组".into())
    })?;

    calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let id = call.get("id").and_then(Value::as_str).ok_or_else(|| {
                AgentMessageConversionError::InvalidToolCalls(format!("第 {index} 项缺少 id"))
            })?;
            let function = call.get("function").ok_or_else(|| {
                AgentMessageConversionError::InvalidToolCalls(format!("第 {index} 项缺少 function"))
            })?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AgentMessageConversionError::InvalidToolCalls(format!(
                        "第 {index} 项缺少 function.name"
                    ))
                })?;
            let arguments = function.get("arguments").cloned().ok_or_else(|| {
                AgentMessageConversionError::InvalidToolCalls(format!(
                    "第 {index} 项缺少 function.arguments"
                ))
            })?;

            Ok(AgentToolCall {
                id: id.to_owned(),
                name: name.to_owned(),
                arguments,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_message_round_trips_typed_tool_calls() {
        let original = ChatMessage {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: Some(serde_json::json!([{
                "id": "call-1",
                "type": "function",
                "function": {
                    "name": "get_time",
                    "arguments": "{}"
                }
            }])),
            tool_call_id: None,
            name: None,
        };

        let typed = agent_message_from_chat(original).unwrap();
        let restored = chat_message_from_agent(typed);

        assert_eq!(restored.role, "assistant");
        assert_eq!(
            restored.tool_calls.unwrap()[0]["function"]["name"],
            "get_time"
        );
    }
}
