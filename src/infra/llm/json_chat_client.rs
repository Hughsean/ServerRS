use std::fmt;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::shared::config::DistillLlmConfig;
use crate::shared::llm_json;

/// OpenAI-compatible JSON 聊天客户端配置。
///
/// 这个类型只表达通用 LLM 传输参数，不绑定 web_ingestion 或 fresh_context
/// 的业务 schema，方便多个蒸馏业务共用同一套 HTTP、鉴权和 JSON 清洗逻辑。
#[derive(Debug, Clone)]
pub struct JsonChatModelConfig {
    pub provider: String,
    pub base_url: String,
    pub chat_model: String,
    pub api_key: String,
    pub temperature: f64,
    pub top_p: f64,
    pub timeout_secs: u64,
}

impl From<DistillLlmConfig> for JsonChatModelConfig {
    fn from(config: DistillLlmConfig) -> Self {
        Self {
            provider: config.provider,
            base_url: config.base_url,
            chat_model: config.chat_model,
            api_key: config.api_key,
            temperature: config.temperature,
            top_p: config.top_p,
            timeout_secs: config.timeout_secs,
        }
    }
}

/// OpenAI-compatible chat message。
#[derive(Debug, Clone, Serialize)]
pub struct JsonChatMessage {
    role: &'static str,
    content: String,
}

impl JsonChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system",
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user",
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct JsonChatResponse<T> {
    pub parsed: T,
    pub llm_input_tokens: Option<u32>,
    pub llm_output_tokens: Option<u32>,
}

#[derive(Debug)]
pub enum JsonChatError {
    ClientBuild(String),
    MissingApiKey,
    Http(String),
    ProviderStatus {
        status: String,
        body_preview: String,
    },
    ResponseJson(String),
    JsonOutput(String),
}

impl fmt::Display for JsonChatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClientBuild(error) => write!(f, "json chat client build failed: {error}"),
            Self::MissingApiKey => write!(f, "json chat api key is empty"),
            Self::Http(error) => write!(f, "json chat HTTP failed: {error}"),
            Self::ProviderStatus {
                status,
                body_preview,
            } => write!(f, "json chat provider returned {status}: {body_preview}"),
            Self::ResponseJson(error) => write!(f, "json chat response JSON failed: {error}"),
            Self::JsonOutput(error) => write!(f, "json chat output JSON failed: {error}"),
        }
    }
}

impl std::error::Error for JsonChatError {}

pub struct OpenAiJsonChatClient {
    client: reqwest::Client,
    config: JsonChatModelConfig,
}

impl OpenAiJsonChatClient {
    pub fn new(config: JsonChatModelConfig) -> Result<Self, JsonChatError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|error| JsonChatError::ClientBuild(error.to_string()))?;
        Ok(Self { client, config })
    }

    /// 调用模型并解析为业务 JSON。若模型首次输出无法解析，会重试一次。
    pub async fn complete_json<T>(
        &self,
        messages: &[JsonChatMessage],
    ) -> Result<JsonChatResponse<T>, JsonChatError>
    where
        T: DeserializeOwned,
    {
        match self.complete_json_once(messages).await {
            Err(JsonChatError::JsonOutput(error)) => {
                tracing::warn!(%error, "LLM JSON 输出解析失败，重试一次");
                self.complete_json_once(messages).await
            }
            result => result,
        }
    }

    async fn complete_json_once<T>(
        &self,
        messages: &[JsonChatMessage],
    ) -> Result<JsonChatResponse<T>, JsonChatError>
    where
        T: DeserializeOwned,
    {
        if self.requires_api_key() && self.request_api_key().is_none() {
            return Err(JsonChatError::MissingApiKey);
        }

        let url = self.chat_completions_url();
        let body = ChatCompletionRequest {
            model: &self.config.chat_model,
            messages,
            temperature: self.config.temperature,
            top_p: self.config.top_p,
        };

        let mut request = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body);
        if let Some(api_key) = self.request_api_key() {
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }

        let response = request
            .send()
            .await
            .map_err(|error| JsonChatError::Http(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(JsonChatError::ProviderStatus {
                status: status.to_string(),
                body_preview: preview(&body),
            });
        }

        let response: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|error| JsonChatError::ResponseJson(error.to_string()))?;
        parse_chat_response(response)
    }

    fn chat_completions_url(&self) -> String {
        chat_completions_url(&self.config.base_url)
    }

    fn request_api_key(&self) -> Option<&str> {
        request_api_key(&self.config)
    }

    fn requires_api_key(&self) -> bool {
        json_chat_requires_api_key(&self.config)
    }
}

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: &'a [JsonChatMessage],
    temperature: f64,
    top_p: f64,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
}

fn parse_chat_response<T>(
    response: ChatCompletionResponse,
) -> Result<JsonChatResponse<T>, JsonChatError>
where
    T: DeserializeOwned,
{
    let input_tokens = response
        .usage
        .as_ref()
        .and_then(|usage| usage.prompt_tokens);
    let output_tokens = response
        .usage
        .as_ref()
        .and_then(|usage| usage.completion_tokens);
    let content = response
        .choices
        .into_iter()
        .next()
        .and_then(|choice| choice.message.content)
        .ok_or_else(|| JsonChatError::JsonOutput("missing choices[0].message.content".into()))?;
    let parsed = llm_json::parse_llm_json::<T>(&content)
        .map_err(|error| JsonChatError::JsonOutput(error.to_string()))?;

    Ok(JsonChatResponse {
        parsed,
        llm_input_tokens: input_tokens,
        llm_output_tokens: output_tokens,
    })
}

fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

fn request_api_key(config: &JsonChatModelConfig) -> Option<&str> {
    let api_key = config.api_key.trim();
    if api_key.is_empty() {
        None
    } else {
        Some(api_key)
    }
}

fn json_chat_requires_api_key(config: &JsonChatModelConfig) -> bool {
    let provider = config.provider.trim().to_ascii_lowercase();
    if provider.contains("ollama") {
        return false;
    }

    let base_url = config.base_url.trim().to_ascii_lowercase();
    !(base_url.starts_with("http://127.0.0.1")
        || base_url.starts_with("http://localhost")
        || base_url.starts_with("http://[::1]"))
}

fn preview(input: &str) -> String {
    const MAX_CHARS: usize = 1024;
    let mut chars = input.chars();
    let preview: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{preview}...[truncated]")
    } else {
        preview
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Decision {
        decision: String,
    }

    #[test]
    fn chat_url_trims_trailing_slashes() {
        assert_eq!(
            chat_completions_url("http://127.0.0.1:11434/v1/"),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
    }

    #[test]
    fn ollama_allows_empty_api_key() {
        let config = JsonChatModelConfig {
            provider: "Ollama".into(),
            base_url: "http://127.0.0.1:11111/v1".into(),
            chat_model: "qwen3".into(),
            api_key: String::new(),
            temperature: 0.1,
            top_p: 0.9,
            timeout_secs: 60,
        };

        assert!(!json_chat_requires_api_key(&config));
        assert_eq!(request_api_key(&config), None);
    }

    #[test]
    fn remote_provider_requires_api_key() {
        let config = JsonChatModelConfig {
            provider: "deepseek".into(),
            base_url: "https://api.deepseek.com".into(),
            chat_model: "deepseek-chat".into(),
            api_key: String::new(),
            temperature: 0.1,
            top_p: 0.9,
            timeout_secs: 60,
        };

        assert!(json_chat_requires_api_key(&config));
        assert_eq!(request_api_key(&config), None);
    }

    #[test]
    fn parses_openai_response_and_qwen_think_output() {
        let response = ChatCompletionResponse {
            choices: vec![ChatChoice {
                message: ChatMessage {
                    content: Some(
                        r#"<think>{"draft":true}</think>
                        {"decision":"publish"}"#
                            .into(),
                    ),
                },
            }],
            usage: Some(Usage {
                prompt_tokens: Some(11),
                completion_tokens: Some(7),
            }),
        };

        let parsed: JsonChatResponse<Decision> = parse_chat_response(response).unwrap();
        assert_eq!(
            parsed.parsed,
            Decision {
                decision: "publish".into()
            }
        );
        assert_eq!(parsed.llm_input_tokens, Some(11));
        assert_eq!(parsed.llm_output_tokens, Some(7));
    }

    #[test]
    fn truncates_provider_error_preview() {
        let long = "x".repeat(1100);
        let shortened = preview(&long);
        assert!(shortened.ends_with("...[truncated]"));
        assert!(shortened.len() < long.len());
    }
}
