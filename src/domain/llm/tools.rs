use async_trait::async_trait;
use serde_json::Value;

// ── Tool outcome ──

/// 调用工具的结果 — 和类型 that makes illegal states unrepresentable.
#[derive(Debug, Clone)]
pub enum ToolOutcome {
    /// Return text directly to the user and stop the tool-calling loop.
    Reply {
        text: String,
        /// Whether the session should be closed after this reply.
        end_session: bool,
    },
    /// Feed this output back to the LLM as a tool result for further processing.
    Continue(String),
}

impl ToolOutcome {
    /// 回复用户而不结束会话。
    pub fn reply(text: impl Into<String>) -> Self {
        Self::Reply {
            text: text.into(),
            end_session: false,
        }
    }

    /// Reply to the user and signal that the session should end.
    pub fn reply_and_end(text: impl Into<String>) -> Self {
        Self::Reply {
            text: text.into(),
            end_session: true,
        }
    }

    /// Feed output back to the LLM for further tool calls or final response.
    pub fn continue_(output: impl Into<String>) -> Self {
        Self::Continue(output.into())
    }
}

// ── Execution context (placeholder for future extension) ──

/// 在工具调用循环中携带的可变状态。
/// Currently a placeholder; tools that need cross-invocation state can
/// extend this struct without changing trait signatures.
#[derive(Debug, Clone, Default)]
pub struct ToolExecutionContext;

// ── Tool trait ──

/// 通过函数调用注册到 LLM 的可调用工具。
#[async_trait]
pub trait LlmTool: Send + Sync {
    /// 用于注册表查找的唯一名称。
    fn name(&self) -> &str;

    /// 兼容 OpenAI 的函数定义（JSON Schema）。
    fn tool_definition(&self) -> Value;

    /// 使用给定参数执行工具。
    ///
    /// `context` is reserved for future cross-invocation state.
    async fn invoke(&self, context: &mut ToolExecutionContext, arguments: &Value) -> ToolOutcome;
}
