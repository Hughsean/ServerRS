use async_trait::async_trait;
use serde::Serialize;
use tracing::{debug, warn};

use crate::domain::llm::{ChatMessage, ChatResponse, LlmClient};

/// Minimal Ollama/OpenAI-compatible chat completion client.
#[derive(Clone)]
pub struct OllamaClient {
    base_url: String,
    model: String,
    temperature: f64,
    top_p: f64,
    client: reqwest::Client,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f64,
    top_p: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

impl OllamaClient {
    pub fn new(base_url: String, model: String, temperature: f64, top_p: f64) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            temperature,
            top_p,
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client should build"),
        }
    }

    /// Raw chat completion with optional tools.
    pub async fn chat_raw(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[serde_json::Value]>,
    ) -> Result<ChatResponse, String> {
        let url = format!("{}/chat/completions", self.base_url);

        let body = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            temperature: self.temperature,
            top_p: self.top_p,
            tools: tools.map(|t| t.to_vec()),
        };

        debug!("Ollama request: {} messages, url={}", messages.len(), url);

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| format!("Ollama request failed: {e}"))?;

        let status = resp.status();
        let body_text = resp.text().await.map_err(|e| format!("read body: {e}"))?;

        if !status.is_success() {
            return Err(format!("Ollama returned {status}: {body_text}"));
        }

        // Clean potential non-JSON prefix
        let cleaned = clean_json_response(&body_text);

        serde_json::from_str::<ChatResponse>(cleaned).map_err(|e| {
            warn!(body = %body_text, "failed to parse Ollama response: {e}");
            format!("parse response: {e}")
        })
    }
}

#[async_trait]
impl LlmClient for OllamaClient {
    async fn chat(&self, messages: &[ChatMessage]) -> String {
        match self.chat_raw(messages, None).await {
            Ok(response) => response
                .choices
                .first()
                .and_then(|c| c.message.content.as_deref())
                .unwrap_or("")
                .to_string(),
            Err(e) => {
                warn!(error = %e, "Ollama unavailable, using fallback");
                "你好呀，我在这里呢。请问有什么可以帮你的吗？".to_string()
            }
        }
    }

    async fn chat_raw(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[serde_json::Value]>,
    ) -> Result<ChatResponse, String> {
        OllamaClient::chat_raw(self, messages, tools).await
    }
}

fn clean_json_response(raw: &str) -> &str {
    if let Some(pos) = raw.find('{') {
        if pos > 0 {
            warn!("cleaned non-JSON prefix from LLM response");
            return &raw[pos..];
        }
    }
    raw
}
