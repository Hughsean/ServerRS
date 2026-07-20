use crate::domain::agent::AgentContext;
use crate::shared::error::AppError;
use crate::shared::llm_json::parse_llm_json;
use serde_json::Value;
use tracing::warn;

/// Agent Runtime 可调用的工具。
#[async_trait::async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    async fn execute(&self, context: &AgentContext, args: Value) -> Result<String, AppError>;
}

/// 一次会话轮次中单个工具调用的兼容追踪记录。
#[derive(Debug, Clone)]
pub struct ToolTrace {
    pub tool_name: String,
    pub arguments: Value,
    pub result: String,
}

/// 规范化 OpenAI 兼容接口返回的工具参数，使工具优先收到 JSON Object。
pub fn normalize_tool_arguments(raw: &Value) -> Value {
    match raw {
        Value::String(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return serde_json::json!({});
            }
            parse_llm_json::<Value>(trimmed).unwrap_or_else(|error| {
                warn!(
                    error = %error,
                    argument_chars = trimmed.chars().count(),
                    "failed to parse tool call arguments; wrapping as error response"
                );
                serde_json::json!({
                    "_invalid_tool_arguments": true,
                    "_raw": trimmed,
                    "_error": error.to_string()
                })
            })
        }
        Value::Null => serde_json::json!({}),
        Value::Object(_) => raw.clone(),
        other => {
            let argument_kind = match other {
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::Array(_) => "array",
                Value::String(_) | Value::Null | Value::Object(_) => "other",
            };
            warn!(
                argument_kind,
                "unexpected tool arguments type; passing through as-is"
            );
            other.clone()
        }
    }
}

pub fn is_tool_call_argument_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("invalid tool call arguments")
        || (lower.contains("400") && lower.contains("bad request") && lower.contains("tool"))
        || lower.contains("tool call arguments")
}

pub(crate) fn truncate_for_event(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        let truncated: String = value.chars().take(max_chars).collect();
        format!("{truncated}...[truncated]")
    }
}
