use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::domain::llm::{
    ChatCompletionRequest, ChatCompletionResponse, ChatMessage, EmbeddingProvider, LlmError,
    LlmProvider, TokenUsage, ToolCall,
};

/// 基于 Ollama（或任意兼容 OpenAI 的端点）的 Provider。
///
/// For chat it sends requests to `POST {base_url}/chat/completions`
/// (OpenAI-compatible).  For embedding it sends to
/// `POST {base_url}/embeddings`（同样兼容 OpenAI）。
///
/// If you need the legacy Ollama-native `/api/chat` or `/api/embeddings`
/// endpoints, you can override `base_url` to omit the `/v1` prefix and
/// change the path -- the request bodies for those endpoints differ, so
/// this struct is **not** a drop-in for the Ollama-native API.
#[derive(Clone)]
pub struct OllamaProvider {
    base_url: String,
    model: String,
    timeout_secs: u64,
    client: reqwest::Client,
}

// ── Request / response shapes for OpenAI-compatible chat ────────────

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f64,
    top_p: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningConfig>,
}

#[derive(Serialize)]
struct ToolDef {
    #[serde(rename = "type")]
    type_: String,
    function: ToolFunction,
}

#[derive(Serialize)]
struct ToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize)]
struct ReasoningConfig {
    enabled: bool,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ResponseToolCall>>,
}

#[derive(Deserialize)]
struct ResponseToolCall {
    id: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    type_: String,
    function: ResponseToolCallFunction,
}

#[derive(Deserialize)]
struct ResponseToolCallFunction {
    name: String,
    arguments: serde_json::Value,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

// ── Request / response shapes for OpenAI-compatible embeddings ──────

#[derive(Serialize)]
struct EmbedRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
}

#[derive(Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
}

// ── Constants ───────────────────────────────────────────────────────

const DEFAULT_TIMEOUT_SECS: u64 = 60;

// ── Impl ────────────────────────────────────────────────────────────

impl OllamaProvider {
    /// 创建一个新的 `OllamaProvider`。
    ///
    /// 使用与现有 `OllamaClient` 相同的 `base_url` 模式，以便
    /// 在迁移期间两者可以共存：
    ///   `http://127.0.0.1:11434/v1`
    pub fn new(base_url: String, model: String, temperature: f64, top_p: f64) -> Self {
        let _ = (temperature, top_p);
        Self::with_timeout(base_url, model, DEFAULT_TIMEOUT_SECS)
    }

    pub fn with_timeout(base_url: String, model: String, timeout_secs: u64) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            timeout_secs,
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest Client should build"),
        }
    }

    /// 聊天补全 URL（兼容 OpenAI）。
    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    /// 嵌入 URL（兼容 OpenAI）。
    fn embed_url(&self) -> String {
        format!("{}/embeddings", self.base_url)
    }

    /// 通用辅助方法：POST JSON，分类错误，反序列化。
    async fn post_json<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        body: &impl Serialize,
    ) -> Result<T, LlmError> {
        let response = self
            .client
            .post(url)
            .json(body)
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::Timeout(format!(
                        "request to {url} timed out after {}s",
                        self.timeout_secs
                    ))
                } else if e.is_connect() {
                    LlmError::Connection(format!("cannot connect to {url}: {e}"))
                } else {
                    LlmError::ProviderError(format!("request to {url} failed: {e}"))
                }
            })?;

        let status = response.status();
        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| LlmError::InvalidResponse(format!("failed to read response body: {e}")))?;

        if !status.is_success() {
            let body_text = String::from_utf8_lossy(&body_bytes);
            let msg = format!("LLM returned {status} from {url}: {body_text}");
            return if status.as_u16() == 429 {
                Err(LlmError::RateLimited(msg))
            } else {
                Err(LlmError::ProviderError(msg))
            };
        }

        serde_json::from_slice(&body_bytes).map_err(|e| {
            let preview = String::from_utf8_lossy(&body_bytes[..body_bytes.len().min(512)]);
            LlmError::InvalidResponse(format!("failed to parse response: {e}, body: {preview}"))
        })
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn chat(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, LlmError> {
        let url = self.chat_url();
        let tools = request.tools.map(|tools| {
            tools
                .into_iter()
                .map(|t| ToolDef {
                    type_: "function".into(),
                    function: ToolFunction {
                        name: t.name,
                        description: t.description,
                        parameters: t.parameters,
                    },
                })
                .collect()
        });

        let body = ChatRequest {
            model: self.model.clone(),
            messages: request.messages,
            temperature: request.temperature,
            top_p: request.top_p,
            max_tokens: request.max_tokens,
            tools,
            reasoning: request.reasoning.map(|r| ReasoningConfig { enabled: r.enabled }),
        };

        debug!("OllamaProvider.chat -> {url}");

        let response: ChatResponse = self.post_json(&url, &body).await?;

        let choice = response.choices.into_iter().next().ok_or_else(|| {
            LlmError::InvalidResponse("chat response contains zero choices".into())
        })?;

        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| ToolCall {
                id: tc.id,
                name: tc.function.name,
                arguments: tc.function.arguments,
            })
            .collect();

        let usage = response.usage.map(|u| TokenUsage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        });

        Ok(ChatCompletionResponse {
            content: choice.message.content.unwrap_or_default(),
            tool_calls,
            finish_reason: choice.finish_reason.unwrap_or_else(|| "stop".into()),
            usage,
        })
    }

    async fn chat_with_tools(
        &self,
        request: ChatCompletionRequest,
        tools: Vec<crate::domain::llm::ToolDefinition>,
    ) -> Result<ChatCompletionResponse, LlmError> {
        let url = self.chat_url();

        let tool_defs: Vec<ToolDef> = tools
            .into_iter()
            .map(|t| ToolDef {
                type_: "function".into(),
                function: ToolFunction {
                    name: t.name,
                    description: t.description,
                    parameters: t.parameters,
                },
            })
            .collect();

        let body = ChatRequest {
            model: self.model.clone(),
            messages: request.messages,
            temperature: request.temperature,
            top_p: request.top_p,
            max_tokens: request.max_tokens,
            tools: Some(tool_defs),
            reasoning: request.reasoning.map(|r| ReasoningConfig { enabled: r.enabled }),
        };

        debug!("OllamaProvider.chat_with_tools -> {url}");

        let response: ChatResponse = self.post_json(&url, &body).await?;

        let choice = response.choices.into_iter().next().ok_or_else(|| {
            LlmError::InvalidResponse("chat with tools response contains zero choices".into())
        })?;

        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| ToolCall {
                id: tc.id,
                name: tc.function.name,
                arguments: tc.function.arguments,
            })
            .collect();

        let usage = response.usage.map(|u| TokenUsage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        });

        Ok(ChatCompletionResponse {
            content: choice.message.content.unwrap_or_default(),
            tool_calls,
            finish_reason: choice.finish_reason.unwrap_or_else(|| "stop".into()),
            usage,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let url = self.embed_url();
        let body = EmbedRequest {
            model: self.model.clone(),
            input: texts.to_vec(),
        };

        debug!("OllamaProvider.embed -> {url} ({} 个文本)", texts.len());

        let response: EmbedResponse = self.post_json(&url, &body).await?;

        if response.data.len() != texts.len() {
            return Err(LlmError::EmbeddingError(format!(
                "expected {} embeddings but got {}",
                texts.len(),
                response.data.len()
            )));
        }

        Ok(response.data.into_iter().map(|d| d.embedding).collect())
    }
}
