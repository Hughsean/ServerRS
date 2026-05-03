use serde::Deserialize;

/// Root configuration — mirrors Java's application.yml
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub jwt: JwtConfig,
    #[serde(default)]
    pub ollama: Option<OllamaConfig>,
    #[serde(default)]
    pub detector: Option<DetectorConfig>,
    #[serde(default)]
    pub session: Option<SessionConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_url")]
    pub url: String,
    #[serde(default = "default_db_max_conn")]
    pub max_connections: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JwtConfig {
    #[serde(default = "default_jwt_secret")]
    pub secret: String,
    /// Token expiration in seconds
    #[serde(default = "default_jwt_expiration")]
    pub expiration_secs: u64,
}

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

#[derive(Debug, Clone, Deserialize)]
pub struct DetectorConfig {
    #[serde(default = "default_context_window")]
    pub context_window_size: usize,
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "default_session_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_ms: u64,
}

// ── Defaults ──

fn default_host() -> String {
    "0.0.0.0".into()
}
fn default_port() -> u16 {
    8080
}
fn default_db_url() -> String {
    "mysql://root:passwd@127.0.0.1:3306/digital_companion".into()
}
fn default_db_max_conn() -> u32 {
    10
}
fn default_jwt_secret() -> String {
    "change-me-in-production-use-a-long-random-string".into()
}
fn default_jwt_expiration() -> u64 {
    86400
}
fn default_ollama_url() -> String {
    "http://10.13.19.91:11434/v1".into()
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
fn default_context_window() -> usize {
    4
}
fn default_confidence_threshold() -> f64 {
    0.5
}
fn default_session_timeout() -> u64 {
    120
}
fn default_cleanup_interval() -> u64 {
    20000
}

impl AppConfig {
    /// Load config from a TOML file path.
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    /// Load config from environment-aware path, falling back to embedded defaults.
    pub fn load() -> Self {
        let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".into());

        match Self::from_file(&path) {
            Ok(cfg) => {
                tracing::info!(path = %path, "configuration loaded");
                cfg
            }
            Err(e) => {
                tracing::warn!(path = %path, error = %e, "failed to load config file, using defaults");
                Self::default()
            }
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: default_host(),
                port: default_port(),
            },
            database: DatabaseConfig {
                url: default_db_url(),
                max_connections: default_db_max_conn(),
            },
            jwt: JwtConfig {
                secret: default_jwt_secret(),
                expiration_secs: default_jwt_expiration(),
            },
            ollama: Some(OllamaConfig {
                base_url: default_ollama_url(),
                model: default_ollama_model(),
                temperature: default_ollama_temperature(),
                top_p: default_ollama_top_p(),
            }),
            detector: Some(DetectorConfig {
                context_window_size: default_context_window(),
                confidence_threshold: default_confidence_threshold(),
            }),
            session: Some(SessionConfig {
                timeout_seconds: default_session_timeout(),
                cleanup_interval_ms: default_cleanup_interval(),
            }),
        }
    }
}
