//! Provider-neutral AI protocols shared by business crates.
//!
//! This crate intentionally contains no HTTP client, provider configuration,
//! persistence implementation, or business prompt.

pub mod chat;
mod json;
pub mod tool;
pub mod tts;

pub use chat::{
    ChatCompletionRequest, ChatCompletionResponse, ChatMessage, EmbeddingProvider, LlmError,
    LlmProvider, ReasoningConfig, TokenUsage, ToolCall, ToolDefinition,
};
pub use json::{clean_llm_json_response, parse_llm_json};
pub use tool::{LlmTool, ToolExecutionContext, ToolOutcome};
pub use tts::{AudioFormat, TtsError, TtsProvider, TtsRequest, TtsResponse};
