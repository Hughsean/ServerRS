//! ## 配置模块
//!
//! 按领域分组为多个子模块，通过 `mod.rs` 重新导出。
//!
//! ### 分组
//!
//! | 文件 | 包含的配置项 |
//! |------|-------------|
//! | `server.rs` | `ServerConfig`, `DatabaseConfig`, `SessionConfig` |
//! | `auth_storage.rs` | `JwtConfig`, `AuthConfig`, `StorageConfig` |
//! | `plugins.rs` | `PluginsConfig` 及 5 个插件配置 |
//! | `mail_cors_log.rs` | `MailConfig`, `CorsConfig`, `LoggingConfig`, `DetectorConfig`, `OllamaConfig` |
//! | `llm_agent_rag.rs` | `LlmConfig`, `AgentConfig`, `RagConfig`, `EmbeddingConfig` |
//! | `web_ingestion.rs` | `WebIngestionConfig`, `DistillLlmConfig` |
//! | `tts.rs` | `TtsConfig` |
//! | `qdrant.rs` | `QdrantConfig` |
//! | `display_config.rs` | `Display for AppConfig` impl |

pub mod auth_storage;
pub mod llm_agent_rag;
pub mod mail_cors_log;
pub mod plugins;
pub mod qdrant;
#[cfg(feature = "qq_bot")]
pub mod qq_bot;
pub mod server;
pub mod ssh;
pub mod tts;
pub mod web_ingestion;

/// 多个子模块共享的默认值辅助函数。
fn default_true() -> bool {
    true
}

use serde::Deserialize;

pub use self::auth_storage::{AuthConfig, JwtConfig, StorageConfig};
pub use self::llm_agent_rag::{AgentConfig, EmbeddingConfig, LlmConfig, RagConfig};
pub use self::mail_cors_log::{
    CorsConfig, DetectorConfig, LoggingConfig, MailConfig, OllamaConfig,
};
pub use self::plugins::{
    BaiduBaikePluginConfig, FetchWebContentPluginConfig, NewsPluginConfig, PluginsConfig,
    WeatherPluginConfig, WebSearchPluginConfig,
};
pub use self::qdrant::QdrantConfig;
#[cfg(feature = "qq_bot")]
pub use self::qq_bot::QqBotConfig;
pub use self::server::{DatabaseConfig, ServerConfig, SessionConfig};
pub use self::ssh::{SshTunnelConfig, TunnelDirection};
pub use self::tts::TtsConfig;
pub use self::web_ingestion::{
    DistillLlmConfig, WebIngestionConfig, WebIngestionHandlerParallelismConfig,
};

// ── AppConfig ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub jwt: JwtConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub ollama: OllamaConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub detector: DetectorConfig,
    #[serde(default)]
    pub plugins: PluginsConfig,
    #[serde(default)]
    pub mail: MailConfig,
    #[serde(default)]
    pub cors: CorsConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub rag: RagConfig,
    #[serde(default)]
    pub qdrant: QdrantConfig,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub web_ingestion: WebIngestionConfig,
    #[serde(default)]
    pub tts: TtsConfig,
    #[serde(default)]
    pub ssh_tunnels: std::collections::HashMap<String, SshTunnelConfig>,
    #[cfg(feature = "qq_bot")]
    #[serde(default)]
    pub qq_bot: QqBotConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: Default::default(),
            database: Default::default(),
            jwt: Default::default(),
            auth: Default::default(),
            storage: Default::default(),
            ollama: Default::default(),
            session: Default::default(),
            detector: Default::default(),
            plugins: Default::default(),
            mail: Default::default(),
            cors: Default::default(),
            logging: Default::default(),
            llm: Default::default(),
            agent: Default::default(),
            rag: Default::default(),
            qdrant: Default::default(),
            embedding: Default::default(),
            web_ingestion: Default::default(),
            tts: Default::default(),
            ssh_tunnels: Default::default(),
            #[cfg(feature = "qq_bot")]
            qq_bot: Default::default(),
        }
    }
}

impl AppConfig {
    /// Load configuration from config file + environment variable overrides.
    pub fn load() -> Self {
        let _ = dotenvy::dotenv();

        let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".into());
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(cfg) => {
                    tracing::info!(path = %path, "配置已加载");
                    cfg
                }
                Err(e) => panic!("failed to parse configuration file {path}: {e}"),
            },
            Err(e) => {
                tracing::warn!(path = %path, error = %e, "未找到配置文件，使用默认配置");
                Self::default()
            }
        };
        cfg.apply_env_overrides();
        cfg.validate()
            .unwrap_or_else(|e| panic!("invalid application configuration: {e}"));
        cfg
    }

    /// Validate configuration consistency.
    pub fn validate(&self) -> Result<(), String> {
        if self.database.url.trim().is_empty() {
            return Err("database.url cannot be empty".into());
        }
        if self.database.max_connections == 0 {
            return Err("database.max_connections must be at least 1".into());
        }
        let default_jwt_secret = "CHANGE_ME_USE_A_LONG_RANDOM_SECRET";
        if self.jwt.secret == default_jwt_secret || self.jwt.secret.len() < 32 {
            return Err(
                "jwt.secret must be replaced with a random value of at least 32 characters".into(),
            );
        }
        if self.jwt.access_ttl_secs == 0 || self.jwt.refresh_ttl_secs == 0 {
            return Err("JWT TTL values must be greater than zero".into());
        }
        if self.rag.chunk_size == 0 || self.rag.chunk_overlap >= self.rag.chunk_size {
            return Err("rag.chunk_overlap must be smaller than rag.chunk_size".into());
        }
        if self.llm.timeout_secs == 0 {
            return Err("llm.timeout_secs must be greater than zero".into());
        }
        if self.embedding.batch_size == 0 || self.embedding.timeout_secs == 0 {
            return Err(
                "embedding.batch_size and embedding.timeout_secs must be greater than zero".into(),
            );
        }
        if !self.rag.hybrid_vector_weight.is_finite()
            || !self.rag.hybrid_keyword_weight.is_finite()
            || self.rag.hybrid_vector_weight < 0.0
            || self.rag.hybrid_keyword_weight < 0.0
            || self.rag.hybrid_vector_weight + self.rag.hybrid_keyword_weight <= 0.0
        {
            return Err("RAG hybrid weights must be non-negative and have a positive sum".into());
        }
        if !self.storage.backend.eq_ignore_ascii_case("LOCAL") {
            return Err(format!(
                "storage.backend={} is not implemented; use LOCAL",
                self.storage.backend
            ));
        }
        Ok(())
    }

    /// Override configuration fields with environment variables.
    /// Does NOT log the actual values for security.
    pub fn apply_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("DATABASE_URL") {
            if !val.is_empty() {
                self.database.url = val;
            }
        }
        if let Ok(val) = std::env::var("AGENT_HTTP_PROXY") {
            self.plugins.fetch_web_content.proxy_url = val.clone();
            self.plugins.baidu_baike.proxy_url = val;
        }
        if let Ok(val) = std::env::var("JWT_SECRET") {
            if !val.is_empty() {
                self.jwt.secret = val;
            }
        }
        if let Ok(val) = std::env::var("WEATHER_API_KEY") {
            if !val.is_empty() {
                self.plugins.weather.api_key = val;
            }
        }
        if let Ok(val) = std::env::var("QDRANT_API_KEY") {
            if !val.is_empty() {
                self.qdrant.api_key = Some(val);
            }
        }
        if let Ok(val) = std::env::var("LLM_BASE_URL") {
            if !val.is_empty() {
                self.llm.base_url = val;
            }
        }
        if let Ok(val) = std::env::var("LLM_CHAT_MODEL") {
            if !val.is_empty() {
                self.llm.chat_model = val;
            }
        }
        if let Ok(val) = std::env::var("LLM_TIMEOUT_SECS") {
            if let Ok(n) = val.parse::<u64>() {
                self.llm.timeout_secs = n;
            }
        }
        if let Ok(val) = std::env::var("EMBEDDING_BASE_URL") {
            if !val.is_empty() {
                self.embedding.base_url = val;
            }
        }
        if let Ok(val) = std::env::var("EMBEDDING_MODEL") {
            if !val.is_empty() {
                self.embedding.model = val;
            }
        }
        if let Ok(val) = std::env::var("EMBEDDING_API_KEY") {
            if !val.is_empty() {
                self.embedding.api_key = val;
            }
        }
        if let Ok(val) = std::env::var("EMBEDDING_DIMENSION") {
            if let Ok(n) = val.parse::<usize>() {
                self.embedding.dimension = n;
            }
        }
        if let Ok(val) = std::env::var("EMBEDDING_BATCH_SIZE") {
            if let Ok(n) = val.parse::<usize>() {
                self.embedding.batch_size = n;
            }
        }
        if let Ok(val) = std::env::var("EMBEDDING_TIMEOUT_SECS") {
            if let Ok(n) = val.parse::<u64>() {
                self.embedding.timeout_secs = n;
            }
        }
        if let Ok(val) = std::env::var("QDRANT_COLLECTION") {
            if !val.is_empty() {
                self.embedding.qdrant_collection = val;
            }
        }

        // ── Web Ingestion ──
        if let Ok(val) = std::env::var("WEB_INGESTION_ENABLED") {
            if let Ok(b) = val.parse::<bool>() {
                self.web_ingestion.enabled = b;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_SCHEDULER_ENABLED") {
            if let Ok(b) = val.parse::<bool>() {
                self.web_ingestion.scheduler_enabled = b;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_DISPATCHER_ENABLED") {
            if let Ok(b) = val.parse::<bool>() {
                self.web_ingestion.dispatcher_enabled = b;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_AUTO_PUBLISH") {
            if let Ok(b) = val.parse::<bool>() {
                self.web_ingestion.auto_publish = b;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_STAGING_REQUIRED") {
            if let Ok(b) = val.parse::<bool>() {
                self.web_ingestion.staging_required = b;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_AUTO_PUBLISH_MIN_SCORE") {
            if let Ok(n) = val.parse::<f64>() {
                self.web_ingestion.auto_publish_min_score = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_PIPELINE_VERSION") {
            if !val.is_empty() {
                self.web_ingestion.pipeline_version = val;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_LLM_PROMPT_VERSION") {
            if !val.is_empty() {
                self.web_ingestion.llm_prompt_version = val;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_CHUNKER_VERSION") {
            if !val.is_empty() {
                self.web_ingestion.chunker_version = val;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_FETCH_TIMEOUT") {
            if let Ok(n) = val.parse::<u64>() {
                self.web_ingestion.fetch_timeout_secs = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_FETCH_USER_AGENT") {
            if !val.is_empty() {
                self.web_ingestion.fetch_user_agent = val;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_FETCH_PROXY_URL") {
            self.web_ingestion.fetch_proxy_url = val;
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_MIN_REQUEST_INTERVAL_MS") {
            if let Ok(n) = val.parse::<u64>() {
                self.web_ingestion.min_request_interval_ms = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_REQUEST_JITTER_MS") {
            if let Ok(n) = val.parse::<u64>() {
                self.web_ingestion.request_jitter_ms = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_MAX_URLS_PER_SOURCE_PER_JOB") {
            if let Ok(n) = val.parse::<usize>() {
                self.web_ingestion.max_urls_per_source_per_job = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_URL_ENQUEUE_DEDUPE_SECS") {
            if let Ok(n) = val.parse::<u64>() {
                self.web_ingestion.url_enqueue_dedupe_secs = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_MAX_BODY_BYTES") {
            if let Ok(n) = val.parse::<u64>() {
                self.web_ingestion.max_body_bytes = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_BATCH_SIZE") {
            if let Ok(n) = val.parse::<usize>() {
                self.web_ingestion.embedding_batch_size = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_QDRANT_COLLECTION") {
            if !val.is_empty() {
                self.web_ingestion.qdrant_collection = val;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_SCHEDULER_INTERVAL_SECS") {
            if let Ok(n) = val.parse::<u64>() {
                self.web_ingestion.scheduler_interval_secs = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_DISPATCHER_INTERVAL_SECS") {
            if let Ok(n) = val.parse::<u64>() {
                self.web_ingestion.dispatcher_interval_secs = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_OUTBOX_BATCH_SIZE") {
            if let Ok(n) = val.parse::<u64>() {
                self.web_ingestion.outbox_batch_size = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_DISPATCHER_PARALLELISM") {
            if let Ok(n) = val.parse::<usize>() {
                self.web_ingestion.dispatcher_parallelism = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_OUTBOX_LOCK_TTL_SECS") {
            if let Ok(n) = val.parse::<u32>() {
                self.web_ingestion.outbox_lock_ttl_secs = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_RETRY_BASE_DELAY_SECS") {
            if let Ok(n) = val.parse::<u64>() {
                self.web_ingestion.retry_base_delay_secs = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_RETRY_MAX_DELAY_SECS") {
            if let Ok(n) = val.parse::<u64>() {
                self.web_ingestion.retry_max_delay_secs = n;
            }
        }
        // ── Distill LLM ──
        if let Ok(val) = std::env::var("WEB_INGESTION_DISTILL_LLM_PROVIDER") {
            if !val.is_empty() {
                self.web_ingestion.distill_llm.provider = val;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_DISTILL_LLM_BASE_URL") {
            if !val.is_empty() {
                self.web_ingestion.distill_llm.base_url = val;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_DISTILL_LLM_CHAT_MODEL") {
            if !val.is_empty() {
                self.web_ingestion.distill_llm.chat_model = val;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_DISTILL_LLM_API_KEY") {
            if !val.is_empty() {
                self.web_ingestion.distill_llm.api_key = val;
            }
        } else if let Ok(val) = std::env::var("DEEPSEEK_API_KEY") {
            if !val.is_empty() {
                self.web_ingestion.distill_llm.api_key = val;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_DISTILL_LLM_TEMPERATURE") {
            if let Ok(n) = val.parse::<f64>() {
                self.web_ingestion.distill_llm.temperature = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_DISTILL_LLM_TOP_P") {
            if let Ok(n) = val.parse::<f64>() {
                self.web_ingestion.distill_llm.top_p = n;
            }
        }
        if let Ok(val) = std::env::var("WEB_INGESTION_DISTILL_LLM_TIMEOUT_SECS") {
            if let Ok(n) = val.parse::<u64>() {
                self.web_ingestion.distill_llm.timeout_secs = n;
            }
        }
        // ── Embedding (separate from distill_llm) ──
        if let Ok(val) = std::env::var("EMBEDDING_PROVIDER") {
            if !val.is_empty() {
                self.embedding.provider = val;
            }
        }
        if let Ok(val) = std::env::var("EMBEDDING_TIMEOUT_SECS") {
            if let Ok(n) = val.parse::<u64>() {
                self.embedding.timeout_secs = n;
            }
        }
        if let Ok(val) = std::env::var("QDRANT_COLLECTION") {
            if !val.is_empty() {
                self.embedding.qdrant_collection = val;
            }
        }
        // ── TTS ──
        if let Ok(val) = std::env::var("TTS_API_KEY") {
            if !val.is_empty() {
                self.tts.api_key = val;
            }
        }
        if let Ok(val) = std::env::var("TTS_RESOURCE_ID") {
            if !val.is_empty() {
                self.tts.resource_id = val;
            }
        }
        if let Ok(val) = std::env::var("TTS_MODEL") {
            if !val.is_empty() {
                self.tts.model = val;
            }
        }
        if let Ok(val) = std::env::var("TTS_DEFAULT_VOICE") {
            if !val.is_empty() {
                self.tts.default_voice = val;
            }
        }
        if let Ok(val) = std::env::var("TTS_DEFAULT_ENCODING") {
            if !val.is_empty() {
                self.tts.default_encoding = val;
            }
        }
        if let Ok(val) = std::env::var("TTS_TIMEOUT_SECS") {
            if let Ok(n) = val.parse::<u64>() {
                self.tts.timeout_secs = n;
            }
        }
        if let Ok(val) = std::env::var("TTS_BASE_URL") {
            if !val.is_empty() {
                self.tts.base_url = val;
            }
        }
        if let Ok(val) = std::env::var("TTS_SAMPLE_RATE") {
            if let Ok(n) = val.parse::<u32>() {
                self.tts.sample_rate = n;
            }
        }
        // ── QQ Bot TTS output ──
        #[cfg(feature = "qq_bot")]
        {
            if let Ok(val) = std::env::var("QQ_BOT_TTS_OUTPUT_DIR") {
                if !val.is_empty() {
                    self.qq_bot.tts_output_dir = val;
                }
            }
            if let Ok(val) = std::env::var("QQ_BOT_TTS_PUBLIC_URL_BASE") {
                if !val.is_empty() {
                    self.qq_bot.tts_public_url_base = val;
                }
            }
        } // cfg(feature = "qq_bot")
    }
}

mod display_config;
