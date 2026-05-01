use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Chat message (moved from infrastructure — belongs in domain).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Port for LLM chat completion — infrastructure provides the real client.
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat(&self, messages: &[ChatMessage]) -> String;
}

/// Port for system prompt templates.
pub trait PromptProvider: Send + Sync {
    fn get_prompt(&self, date_time: &str) -> String;
}
