use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum ToolOutcome {
    Reply { text: String, end_session: bool },
    Continue(String),
}

impl ToolOutcome {
    pub fn reply(text: impl Into<String>) -> Self {
        Self::Reply {
            text: text.into(),
            end_session: false,
        }
    }

    pub fn reply_and_end(text: impl Into<String>) -> Self {
        Self::Reply {
            text: text.into(),
            end_session: true,
        }
    }

    pub fn continue_(output: impl Into<String>) -> Self {
        Self::Continue(output.into())
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolExecutionContext;

#[async_trait]
pub trait LlmTool: Send + Sync {
    fn name(&self) -> &str;
    fn tool_definition(&self) -> Value;
    async fn invoke(&self, context: &mut ToolExecutionContext, arguments: &Value) -> ToolOutcome;
}
