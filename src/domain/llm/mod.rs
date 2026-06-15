pub mod tools;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ── Core types ─────────────────────────────────────────────────────────────────

/// Chat message — used by both legacy LlmClient and new LlmProvider.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: ChoiceMessage,
}

#[derive(Debug, Deserialize)]
pub struct ChoiceMessage {
    pub content: Option<String>,
    pub tool_calls: Option<serde_json::Value>,
}

// ── Legacy LLM client trait (used by DiaryService for title generation) ──────

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat(&self, messages: &[ChatMessage]) -> String;
    async fn chat_raw(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[serde_json::Value]>,
    ) -> Result<ChatResponse, String>;
}

pub trait PromptProvider: Send + Sync {
    fn get_prompt(&self, date_time: &str) -> String;
}

// ── New LlmProvider / EmbeddingProvider (Agent upgrade) ────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct ChatCompletionRequest {
    pub messages: Vec<ChatMessage>,
    pub temperature: f64,
    pub top_p: f64,
    pub max_tokens: Option<u32>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub reasoning: Option<ReasoningConfig>,
}

impl ChatCompletionRequest {
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            temperature: 0.7,
            top_p: 0.9,
            max_tokens: None,
            tools: None,
            reasoning: None,
        }
    }
    pub fn with_temperature(mut self, t: f64) -> Self {
        self.temperature = t;
        self
    }
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }
}

#[derive(Debug, Clone)]
pub struct ChatCompletionResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone)]
pub enum LlmError {
    Connection(String),
    Timeout(String),
    RateLimited(String),
    InvalidResponse(String),
    ProviderError(String),
    EmbeddingError(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection(m) => write!(f, "LLM connection error: {m}"),
            Self::Timeout(m) => write!(f, "LLM timeout: {m}"),
            Self::RateLimited(m) => write!(f, "LLM rate limited: {m}"),
            Self::InvalidResponse(m) => write!(f, "LLM invalid response: {m}"),
            Self::ProviderError(m) => write!(f, "LLM provider error: {m}"),
            Self::EmbeddingError(m) => write!(f, "Embedding error: {m}"),
        }
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, LlmError>;
    async fn chat_with_tools(
        &self,
        request: ChatCompletionRequest,
        tools: Vec<ToolDefinition>,
    ) -> Result<ChatCompletionResponse, LlmError>;
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError>;
}
