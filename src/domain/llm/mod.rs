use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod tools;

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

/// Port for LLM chat completion — infrastructure provides the real client.
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat(&self, messages: &[ChatMessage]) -> String;
    async fn chat_raw(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[serde_json::Value]>,
    ) -> Result<ChatResponse, String>;
}

/// Port for system prompt templates.
pub trait PromptProvider: Send + Sync {
    fn get_prompt(&self, date_time: &str) -> String;
}
