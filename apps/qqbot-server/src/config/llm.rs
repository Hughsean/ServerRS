//! QQBot 独立 LLM 配置。
//!
//! API Key 只允许来自进程环境或本地文件，禁止写入 TOML。
//! 当前垂直切片仅用于有界线程语义提取，不允许模型直接访问数据库、网络工具或消息发送。

use std::path::PathBuf;

use serde::Deserialize;

use super::ConfigError;
use super::validation::is_loopback_host;

pub(crate) const DEFAULT_OPENAI_COMPATIBLE_BASE_URL: &str = "http://127.0.0.1:11434/v1";
pub(crate) const DEEPSEEK_OFFICIAL_BASE_URL: &str = "https://api.deepseek.com/v1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LlmConfig {
    pub enabled: bool,
    pub provider: LlmProvider,
    pub base_url: String,
    pub model: String,
    pub api_key_file: Option<PathBuf>,
    pub connect_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub max_input_chars: usize,
    pub max_output_tokens: u32,
    pub max_response_bytes: usize,
    pub temperature: f64,
    pub max_candidates_per_kind: usize,
    pub reasoning_mode: LlmReasoningMode,
    /// 可选的每百万 token 微美元单价；两项同时配置后才估算成本。
    pub input_cost_microusd_per_million_tokens: Option<u64>,
    pub output_cost_microusd_per_million_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum LlmProvider {
    #[default]
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    #[serde(rename = "deepseek")]
    DeepSeek,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmReasoningMode {
    #[default]
    ProviderDefault,
    QwenNoThink,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: LlmProvider::OpenAiCompatible,
            base_url: DEFAULT_OPENAI_COMPATIBLE_BASE_URL.into(),
            model: String::new(),
            api_key_file: None,
            connect_timeout_secs: 10,
            request_timeout_secs: 60,
            max_input_chars: 60_000,
            max_output_tokens: 2_000,
            max_response_bytes: 1_048_576,
            temperature: 0.1,
            max_candidates_per_kind: 20,
            reasoning_mode: LlmReasoningMode::ProviderDefault,
            input_cost_microusd_per_million_tokens: None,
            output_cost_microusd_per_million_tokens: None,
        }
    }
}

impl LlmConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if !self.enabled {
            return Ok(());
        }
        if self.model.trim().is_empty() || self.model.len() > 191 {
            return Err(ConfigError::Invalid(
                "llm.model must contain 1..=191 bytes when enabled".into(),
            ));
        }
        if self.provider == LlmProvider::DeepSeek
            && self.reasoning_mode != LlmReasoningMode::ProviderDefault
        {
            return Err(ConfigError::Invalid(
                "llm.reasoning_mode must be provider_default for the DeepSeek provider".into(),
            ));
        }
        if self.provider == LlmProvider::DeepSeek
            && self.base_url.trim_end_matches('/') != DEFAULT_OPENAI_COMPATIBLE_BASE_URL
            && self.base_url.trim_end_matches('/') != DEEPSEEK_OFFICIAL_BASE_URL
        {
            return Err(ConfigError::Invalid(
                "llm.base_url cannot override the official DeepSeek endpoint".into(),
            ));
        }
        let effective_base_url = self.effective_base_url();
        let url = url::Url::parse(effective_base_url).map_err(|error| {
            ConfigError::Invalid(format!("llm.base_url must be an absolute URL: {error}"))
        })?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ConfigError::Invalid(
                "llm.base_url must not contain credentials, query, or fragment".into(),
            ));
        }
        match url.scheme() {
            "https" => {}
            "http" if url.host_str().is_some_and(is_loopback_host) => {}
            _ => {
                return Err(ConfigError::Invalid(
                    "llm.base_url must use HTTPS; plain HTTP is allowed only on loopback".into(),
                ));
            }
        }
        if !(1..=300).contains(&self.connect_timeout_secs)
            || !(1..=600).contains(&self.request_timeout_secs)
        {
            return Err(ConfigError::Invalid(
                "llm timeouts must be positive and bounded".into(),
            ));
        }
        if !(1_000..=1_000_000).contains(&self.max_input_chars) {
            return Err(ConfigError::Invalid(
                "llm.max_input_chars must be in 1000..=1000000".into(),
            ));
        }
        if !(1..=32_768).contains(&self.max_output_tokens) {
            return Err(ConfigError::Invalid(
                "llm.max_output_tokens must be in 1..=32768".into(),
            ));
        }
        if !(1_024..=10_485_760).contains(&self.max_response_bytes) {
            return Err(ConfigError::Invalid(
                "llm.max_response_bytes must be in 1024..=10485760".into(),
            ));
        }
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err(ConfigError::Invalid(
                "llm.temperature must be finite and in 0..=2".into(),
            ));
        }
        if !(1..=100).contains(&self.max_candidates_per_kind) {
            return Err(ConfigError::Invalid(
                "llm.max_candidates_per_kind must be in 1..=100".into(),
            ));
        }
        if self.input_cost_microusd_per_million_tokens.is_some()
            != self.output_cost_microusd_per_million_tokens.is_some()
        {
            return Err(ConfigError::Invalid(
                "llm input/output cost prices must be configured together".into(),
            ));
        }
        if self
            .input_cost_microusd_per_million_tokens
            .into_iter()
            .chain(self.output_cost_microusd_per_million_tokens)
            .any(|value| value > 1_000_000_000_000)
        {
            return Err(ConfigError::Invalid(
                "llm token cost price must be <= 1000000000000 microusd per million tokens".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn effective_base_url(&self) -> &str {
        match self.provider {
            LlmProvider::OpenAiCompatible => &self.base_url,
            LlmProvider::DeepSeek => DEEPSEEK_OFFICIAL_BASE_URL,
        }
    }

    pub(crate) fn api_key(&self) -> Result<Option<String>, ConfigError> {
        let key_env = match self.provider {
            LlmProvider::OpenAiCompatible => "QQBOT_LLM_API_KEY",
            LlmProvider::DeepSeek => "QQBOT_DEEPSEEK_API_KEY",
        };
        if let Ok(value) = std::env::var(key_env)
            && !value.trim().is_empty()
        {
            return Ok(Some(value));
        }
        let Some(path) = &self.api_key_file else {
            return Ok(None);
        };
        let value = std::fs::read_to_string(path).map_err(|error| {
            ConfigError::Invalid(format!("failed to read llm.api_key_file: {error}"))
        })?;
        let value = value.trim().to_owned();
        if value.is_empty() {
            return Err(ConfigError::Invalid("llm.api_key_file is empty".into()));
        }
        Ok(Some(value))
    }
}
