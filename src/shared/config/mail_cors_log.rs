use serde::Deserialize;

// ── OllamaConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct OllamaConfig {
    #[serde(default = "default_ollama_url")]
    pub base_url: String,
    #[serde(default = "default_ollama_model")]
    pub model: String,
    #[serde(default = "default_ollama_temperature")]
    pub temperature: f64,
    #[serde(default = "default_ollama_top_p")]
    pub top_p: f64,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: default_ollama_url(),
            model: default_ollama_model(),
            temperature: default_ollama_temperature(),
            top_p: default_ollama_top_p(),
        }
    }
}

fn default_ollama_url() -> String {
    "http://127.0.0.1:11434/v1".into()
}
fn default_ollama_model() -> String {
    "qwen2.5:14b".into()
}
fn default_ollama_temperature() -> f64 {
    0.5
}
fn default_ollama_top_p() -> f64 {
    0.9
}

// ── DetectorConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct DetectorConfig {
    #[serde(default = "default_context_window")]
    pub context_window_size: usize,
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
    #[serde(default)]
    pub llm_enabled: bool,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            context_window_size: default_context_window(),
            confidence_threshold: default_confidence_threshold(),
            llm_enabled: false,
            max_retries: default_max_retries(),
        }
    }
}

fn default_context_window() -> usize {
    4
}
fn default_confidence_threshold() -> f64 {
    0.5
}
fn default_max_retries() -> u32 {
    3
}

// ── MailConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct MailConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_mail_host")]
    pub host: String,
    #[serde(default = "default_mail_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_mail_from")]
    pub from_address: String,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_mail_host(),
            port: default_mail_port(),
            username: String::new(),
            password: String::new(),
            from_address: default_mail_from(),
        }
    }
}

fn default_mail_host() -> String {
    "smtp.example.com".into()
}
fn default_mail_port() -> u16 {
    587
}
fn default_mail_from() -> String {
    "noreply@example.com".into()
}

// ── CorsConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct CorsConfig {
    #[serde(default = "default_allowed_origins")]
    pub allowed_origins: Vec<String>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: default_allowed_origins(),
        }
    }
}

fn default_allowed_origins() -> Vec<String> {
    vec!["http://localhost:3000".into()]
}

// ── LoggingConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
}

fn default_log_level() -> String {
    "info".into()
}
