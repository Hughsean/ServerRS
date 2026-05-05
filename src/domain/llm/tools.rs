use async_trait::async_trait;
use serde_json::Value;

// ── Tool outcome ──

/// The result of invoking a tool — a sum type that makes illegal states unrepresentable.
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
    /// Reply to the user without ending the session.
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

/// Mutable state carried through a tool-calling loop.
/// Currently a placeholder; tools that need cross-invocation state can
/// extend this struct without changing trait signatures.
#[derive(Debug, Clone, Default)]
pub struct ToolExecutionContext;

// ── Tool trait ──

/// A callable tool registered with the LLM via function-calling.
#[async_trait]
pub trait LlmTool: Send + Sync {
    /// Unique name used for registry lookup.
    fn name(&self) -> &str;

    /// OpenAI-compatible function definition (JSON schema).
    fn tool_definition(&self) -> Value;

    /// Execute the tool with the given arguments.
    ///
    /// `context` is reserved for future cross-invocation state.
    async fn invoke(&self, context: &mut ToolExecutionContext, arguments: &Value) -> ToolOutcome;
}
