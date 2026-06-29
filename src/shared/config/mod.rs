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
pub mod fresh_context;
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

use std::collections::BTreeSet;

use serde::Deserialize;

pub use self::auth_storage::{AuthConfig, JwtConfig, StorageConfig};
pub use self::fresh_context::FreshContextConfig;
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
    pub fresh_context: FreshContextConfig,
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
            fresh_context: Default::default(),
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
        cfg.resolve_tunnel_templates()
            .unwrap_or_else(|e| panic!("invalid application configuration: {e}"));
        cfg.validate()
            .unwrap_or_else(|e| panic!("invalid application configuration: {e}"));
        cfg
    }

    /// Validate configuration consistency.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_tunnel_references()?;
        validate_required_url(&self.database.url, "database.url")?;
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
        validate_required_url(&self.llm.base_url, "llm.base_url")?;
        if self.llm.timeout_secs == 0 {
            return Err("llm.timeout_secs must be greater than zero".into());
        }
        validate_required_url(&self.embedding.base_url, "embedding.base_url")?;
        if self.embedding.batch_size == 0 || self.embedding.timeout_secs == 0 {
            return Err(
                "embedding.batch_size and embedding.timeout_secs must be greater than zero".into(),
            );
        }
        if self.qdrant.enabled {
            validate_required_url(&self.qdrant.url, "qdrant.url")?;
        }
        if self.web_ingestion.enabled
            && (self.web_ingestion.scheduler_enabled || self.web_ingestion.dispatcher_enabled)
        {
            validate_required_url(
                &self.web_ingestion.distill_llm.base_url,
                "web_ingestion.distill_llm.base_url",
            )?;
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
        if self.fresh_context.scheduler_interval_secs == 0
            || self.fresh_context.dispatcher_interval_secs == 0
            || self.fresh_context.fetch_timeout_secs == 0
            || self.fresh_context.max_sources_per_tick == 0
            || self.fresh_context.max_items_per_source == 0
            || self.fresh_context.max_pipeline_items_per_tick == 0
            || self.fresh_context.chunk_size == 0
            || self.fresh_context.max_indexable_chunks_per_tick == 0
            || self.fresh_context.max_topic_items_per_tick == 0
            || self.fresh_context.max_expired_vectors_per_tick == 0
            || self.fresh_context.max_retrieval_chunks == 0
        {
            return Err("fresh_context intervals and limits must be greater than zero".into());
        }
        if self.fresh_context.chunk_overlap >= self.fresh_context.chunk_size {
            return Err("fresh_context.chunk_overlap must be less than chunk_size".into());
        }
        if self.fresh_context.trend_ttl_secs == 0
            || self.fresh_context.gossip_ttl_secs == 0
            || self.fresh_context.news_ttl_secs == 0
            || self.fresh_context.background_ttl_secs == 0
        {
            return Err("fresh_context TTL values must be greater than zero".into());
        }
        let fresh_weight_sum = self.fresh_context.semantic_weight
            + self.fresh_context.freshness_weight
            + self.fresh_context.reliability_weight
            + self.fresh_context.heat_weight;
        if !fresh_weight_sum.is_finite()
            || fresh_weight_sum <= 0.0
            || self.fresh_context.semantic_weight < 0.0
            || self.fresh_context.freshness_weight < 0.0
            || self.fresh_context.reliability_weight < 0.0
            || self.fresh_context.heat_weight < 0.0
        {
            return Err(
                "fresh_context ranking weights must be non-negative and sum to a positive value"
                    .into(),
            );
        }
        if !(0.0..=1.0).contains(&self.fresh_context.min_reliability_score) {
            return Err("fresh_context.min_reliability_score must be between 0.0 and 1.0".into());
        }
        if self.fresh_context.distill_llm.timeout_secs == 0 {
            return Err("fresh_context.distill_llm.timeout_secs must be greater than zero".into());
        }
        if self.fresh_context.enabled && self.fresh_context.dispatcher_enabled {
            validate_required_url(
                &self.fresh_context.distill_llm.base_url,
                "fresh_context.distill_llm.base_url",
            )?;
        }
        Ok(())
    }

    /// 返回所有被业务配置引用的 SSH 隧道名称。
    ///
    /// 启动层只需要关心这个集合，不需要逐个扫描 database/llm/qdrant 等配置段。
    pub fn referenced_ssh_tunnel_names(&self) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        push_tunnel_name(&mut names, self.database.tunnel.as_deref());
        push_tunnel_name(&mut names, self.ollama.tunnel.as_deref());
        push_tunnel_name(&mut names, self.llm.tunnel.as_deref());
        push_tunnel_name(&mut names, self.embedding.tunnel.as_deref());
        push_tunnel_name(&mut names, self.qdrant.tunnel.as_deref());
        push_tunnel_name(&mut names, self.web_ingestion.distill_llm.tunnel.as_deref());
        push_tunnel_name(&mut names, self.fresh_context.distill_llm.tunnel.as_deref());
        names
    }

    /// 返回启动时真正需要拉起的 SSH 隧道。
    ///
    /// 远程转发隧道用于对外暴露端口，保持无条件启动；本地转发仅在被配置引用时启动。
    pub fn active_ssh_tunnels(&self) -> Vec<(String, SshTunnelConfig)> {
        let referenced = self.referenced_ssh_tunnel_names();
        self.ssh_tunnels
            .iter()
            .filter(|(name, cfg)| {
                matches!(cfg.direction, TunnelDirection::Remote) || referenced.contains(*name)
            })
            .map(|(name, cfg)| (name.clone(), cfg.clone()))
            .collect()
    }

    fn validate_tunnel_references(&self) -> Result<(), String> {
        for name in self.referenced_ssh_tunnel_names() {
            if !self.ssh_tunnels.contains_key(&name) {
                return Err(format!("ssh tunnel '{name}' is referenced but not defined"));
            }
        }
        Ok(())
    }

    /// 根据本地 SSH 隧道端点替换 URL 模板占位符。
    ///
    /// 有 tunnel 的字段必须显式写 `{ip}` 和 `{port}`，后端只做占位符替换。
    /// 没有 tunnel 的字段按普通 URL 使用，不允许残留 `{ip}` / `{port}`。
    fn resolve_tunnel_templates(&mut self) -> Result<(), String> {
        if let Some(url) = render_tunnel_template(
            "database.url",
            &self.ssh_tunnels,
            self.database.tunnel.as_deref(),
            &self.database.url,
        )? {
            self.database.url = url;
        }

        if let Some(url) = render_tunnel_template(
            "ollama.base_url",
            &self.ssh_tunnels,
            self.ollama.tunnel.as_deref(),
            &self.ollama.base_url,
        )? {
            self.ollama.base_url = url;
        }

        if let Some(url) = render_tunnel_template(
            "llm.base_url",
            &self.ssh_tunnels,
            self.llm.tunnel.as_deref(),
            &self.llm.base_url,
        )? {
            self.llm.base_url = url;
        }

        if let Some(url) = render_tunnel_template(
            "embedding.base_url",
            &self.ssh_tunnels,
            self.embedding.tunnel.as_deref(),
            &self.embedding.base_url,
        )? {
            self.embedding.base_url = url;
        }

        if let Some(url) = render_tunnel_template(
            "qdrant.url",
            &self.ssh_tunnels,
            self.qdrant.tunnel.as_deref(),
            &self.qdrant.url,
        )? {
            self.qdrant.url = url;
        }

        if let Some(url) = render_tunnel_template(
            "web_ingestion.distill_llm.base_url",
            &self.ssh_tunnels,
            self.web_ingestion.distill_llm.tunnel.as_deref(),
            &self.web_ingestion.distill_llm.base_url,
        )? {
            self.web_ingestion.distill_llm.base_url = url;
        }
        if let Some(url) = render_tunnel_template(
            "fresh_context.distill_llm.base_url",
            &self.ssh_tunnels,
            self.fresh_context.distill_llm.tunnel.as_deref(),
            &self.fresh_context.distill_llm.base_url,
        )? {
            self.fresh_context.distill_llm.base_url = url;
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
        // ── Fresh Context Distill LLM ──
        if let Ok(val) = std::env::var("FRESH_CONTEXT_DISTILL_LLM_PROVIDER") {
            if !val.is_empty() {
                self.fresh_context.distill_llm.provider = val;
            }
        }
        if let Ok(val) = std::env::var("FRESH_CONTEXT_DISTILL_LLM_BASE_URL") {
            if !val.is_empty() {
                self.fresh_context.distill_llm.base_url = val;
            }
        }
        if let Ok(val) = std::env::var("FRESH_CONTEXT_DISTILL_LLM_CHAT_MODEL") {
            if !val.is_empty() {
                self.fresh_context.distill_llm.chat_model = val;
            }
        }
        if let Ok(val) = std::env::var("FRESH_CONTEXT_DISTILL_LLM_API_KEY") {
            if !val.is_empty() {
                self.fresh_context.distill_llm.api_key = val;
            }
        } else if let Ok(val) = std::env::var("DEEPSEEK_API_KEY") {
            if !val.is_empty() {
                self.fresh_context.distill_llm.api_key = val;
            }
        }
        if let Ok(val) = std::env::var("FRESH_CONTEXT_DISTILL_LLM_TEMPERATURE") {
            if let Ok(n) = val.parse::<f64>() {
                self.fresh_context.distill_llm.temperature = n;
            }
        }
        if let Ok(val) = std::env::var("FRESH_CONTEXT_DISTILL_LLM_TOP_P") {
            if let Ok(n) = val.parse::<f64>() {
                self.fresh_context.distill_llm.top_p = n;
            }
        }
        if let Ok(val) = std::env::var("FRESH_CONTEXT_DISTILL_LLM_TIMEOUT_SECS") {
            if let Ok(n) = val.parse::<u64>() {
                self.fresh_context.distill_llm.timeout_secs = n;
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

fn push_tunnel_name(names: &mut BTreeSet<String>, name: Option<&str>) {
    if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) {
        names.insert(name.to_string());
    }
}

const TUNNEL_TEMPLATE_IP: &str = "{ip}";
const TUNNEL_TEMPLATE_PORT: &str = "{port}";

fn render_tunnel_template(
    field_name: &str,
    tunnels: &std::collections::HashMap<String, SshTunnelConfig>,
    tunnel_name: Option<&str>,
    template_url: &str,
) -> Result<Option<String>, String> {
    let Some(name) = tunnel_name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(None);
    };
    let template = template_url.trim();
    if template.is_empty() {
        return Err(format!(
            "{field_name} cannot be empty when tunnel '{name}' is set; use {TUNNEL_TEMPLATE_IP} and {TUNNEL_TEMPLATE_PORT} placeholders"
        ));
    }
    if !template.contains(TUNNEL_TEMPLATE_IP) || !template.contains(TUNNEL_TEMPLATE_PORT) {
        return Err(format!(
            "{field_name} must contain {TUNNEL_TEMPLATE_IP} and {TUNNEL_TEMPLATE_PORT} when tunnel '{name}' is set"
        ));
    }
    let tunnel = tunnels
        .get(name)
        .ok_or_else(|| format!("ssh tunnel '{name}' is referenced but not defined"))?;
    if !matches!(tunnel.direction, TunnelDirection::Local) {
        return Err(format!(
            "{field_name} references ssh tunnel '{name}', but URL rewriting requires a local tunnel"
        ));
    }
    let host = tunnel
        .bind_address
        .as_deref()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .unwrap_or("127.0.0.1");
    let rendered = template
        .replace(TUNNEL_TEMPLATE_IP, host)
        .replace(TUNNEL_TEMPLATE_PORT, &tunnel.local_port.to_string());
    parse_url(&rendered, field_name)?;
    Ok(Some(rendered))
}

fn validate_required_url(value: &str, field_name: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!(
            "{field_name} cannot be empty; set it to a URL template"
        ));
    }
    if contains_tunnel_template_placeholder(value) {
        return Err(format!(
            "{field_name} contains unresolved {TUNNEL_TEMPLATE_IP}/{TUNNEL_TEMPLATE_PORT} placeholders; configure tunnel or use a concrete URL"
        ));
    }
    parse_url(value, field_name)?;
    Ok(())
}

fn parse_url(template: &str, field_name: &str) -> Result<reqwest::Url, String> {
    reqwest::Url::parse(template)
        .map_err(|error| format!("{field_name} must be a valid absolute URL: {error}"))
}

fn contains_tunnel_template_placeholder(value: &str) -> bool {
    value.contains(TUNNEL_TEMPLATE_IP) || value.contains(TUNNEL_TEMPLATE_PORT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_secret() -> String {
        "01234567890123456789012345678901".into()
    }

    #[test]
    fn internal_default_config_keeps_url_fields_empty() {
        let config = AppConfig::default();

        assert!(config.database.url.is_empty());
        assert!(config.ollama.base_url.is_empty());
        assert!(config.llm.base_url.is_empty());
        assert!(config.embedding.base_url.is_empty());
        assert!(config.qdrant.url.is_empty());
        assert!(config.web_ingestion.distill_llm.base_url.is_empty());
        assert!(config.fresh_context.distill_llm.base_url.is_empty());
    }

    #[test]
    fn renders_llm_url_template_from_tunnel() {
        let raw = r#"
            [ssh_tunnels.ollama]
            host = "host-a"
            local_port = 11111
            remote_port = 11434

            [llm]
            base_url = "https://{ip}:{port}/v1"
            tunnel = "ollama"
        "#;
        let mut config: AppConfig = toml::from_str(raw).unwrap();

        config.resolve_tunnel_templates().unwrap();

        assert_eq!(config.llm.base_url, "https://127.0.0.1:11111/v1");
        assert_eq!(
            config.referenced_ssh_tunnel_names(),
            BTreeSet::from(["ollama".to_string()])
        );
    }

    #[test]
    fn tunnel_requires_url_template_field() {
        let raw = r#"
            [ssh_tunnels.ollama]
            host = "host-a"
            local_port = 11111
            remote_port = 11434

            [llm]
            tunnel = "ollama"
        "#;

        let error = toml::from_str::<AppConfig>(raw).unwrap_err().to_string();

        assert!(error.contains("missing field `base_url`"));
    }

    #[test]
    fn tunnel_requires_ip_and_port_placeholders() {
        let raw = r#"
            [ssh_tunnels.ollama]
            host = "host-a"
            local_port = 11111
            remote_port = 11434

            [llm]
            base_url = "http://127.0.0.1:11434/v1"
            tunnel = "ollama"
        "#;
        let mut config: AppConfig = toml::from_str(raw).unwrap();

        let error = config.resolve_tunnel_templates().unwrap_err();

        assert!(error.contains("llm.base_url must contain {ip} and {port}"));
    }

    #[test]
    fn renders_database_url_template_from_tunnel() {
        let raw = r#"
            [ssh_tunnels.mysql]
            host = "host-a"
            local_port = 13306
            remote_port = 3306

            [database]
            url = "mysql://root:password@{ip}:{port}/digital_companion"
            tunnel = "mysql"
        "#;
        let mut config: AppConfig = toml::from_str(raw).unwrap();

        config.resolve_tunnel_templates().unwrap();

        assert_eq!(
            config.database.url,
            "mysql://root:password@127.0.0.1:13306/digital_companion"
        );
    }

    #[test]
    fn renders_embedding_distill_and_qdrant_templates_from_tunnels() {
        let raw = r#"
            [ssh_tunnels.ollama]
            host = "host-a"
            local_port = 11111
            remote_port = 11434

            [ssh_tunnels.qdrant]
            host = "host-b"
            local_port = 6334
            remote_port = 6333

            [embedding]
            base_url = "http://{ip}:{port}/v1"
            tunnel = "ollama"

            [web_ingestion.distill_llm]
            base_url = "http://{ip}:{port}/v1"
            tunnel = "ollama"

            [fresh_context.distill_llm]
            base_url = "http://{ip}:{port}/v1"
            tunnel = "ollama"

            [qdrant]
            url = "http://{ip}:{port}"
            tunnel = "qdrant"
        "#;
        let mut config: AppConfig = toml::from_str(raw).unwrap();

        config.resolve_tunnel_templates().unwrap();

        assert_eq!(config.embedding.base_url, "http://127.0.0.1:11111/v1");
        assert_eq!(
            config.web_ingestion.distill_llm.base_url,
            "http://127.0.0.1:11111/v1"
        );
        assert_eq!(
            config.fresh_context.distill_llm.base_url,
            "http://127.0.0.1:11111/v1"
        );
        assert_eq!(config.qdrant.url, "http://127.0.0.1:6334");
    }

    #[test]
    fn tunnel_template_preserves_url_structure() {
        let raw = r#"
            [ssh_tunnels.ollama]
            host = "host-a"
            local_port = 11111
            remote_port = 11434

            [llm]
            base_url = "https://{ip}:{port}/custom/v2?tenant=alpha"
            tunnel = "ollama"
        "#;
        let mut config: AppConfig = toml::from_str(raw).unwrap();

        config.resolve_tunnel_templates().unwrap();

        assert_eq!(
            config.llm.base_url,
            "https://127.0.0.1:11111/custom/v2?tenant=alpha"
        );
    }

    #[test]
    fn validate_accepts_tunnel_resolved_urls() {
        let raw = r#"
            [ssh_tunnels.mysql]
            host = "host-db"
            local_port = 13306
            remote_port = 3306

            [database]
            url = "mysql://root:password@{ip}:{port}/digital_companion"
            tunnel = "mysql"

            [jwt]
            secret = "01234567890123456789012345678901"

            [ssh_tunnels.ollama]
            host = "host-a"
            local_port = 11111
            remote_port = 11434

            [ssh_tunnels.qdrant]
            host = "host-b"
            local_port = 6334
            remote_port = 6333

            [llm]
            base_url = "http://{ip}:{port}/v1"
            tunnel = "ollama"

            [embedding]
            base_url = "http://{ip}:{port}/v1"
            tunnel = "ollama"

            [qdrant]
            enabled = true
            url = "http://{ip}:{port}"
            tunnel = "qdrant"
        "#;
        let mut config: AppConfig = toml::from_str(raw).unwrap();

        config.resolve_tunnel_templates().unwrap();

        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_unknown_tunnel_reference() {
        let mut config = AppConfig::default();
        config.jwt.secret = test_secret();
        config.llm.tunnel = Some("missing".into());

        let error = config.validate().unwrap_err();

        assert!(error.contains("ssh tunnel 'missing' is referenced but not defined"));
    }

    #[test]
    fn active_tunnels_include_referenced_local_and_all_remote() {
        let mut config = AppConfig::default();
        config.llm.tunnel = Some("ollama".into());
        config.ssh_tunnels.insert(
            "ollama".into(),
            SshTunnelConfig {
                host: "host-a".into(),
                user: None,
                local_port: 11111,
                remote_port: 11434,
                direction: TunnelDirection::Local,
                bind_address: None,
            },
        );
        config.ssh_tunnels.insert(
            "unused".into(),
            SshTunnelConfig {
                host: "host-b".into(),
                user: None,
                local_port: 22222,
                remote_port: 22222,
                direction: TunnelDirection::Local,
                bind_address: None,
            },
        );
        config.ssh_tunnels.insert(
            "public".into(),
            SshTunnelConfig {
                host: "host-c".into(),
                user: None,
                local_port: 8080,
                remote_port: 8080,
                direction: TunnelDirection::Remote,
                bind_address: Some("0.0.0.0".into()),
            },
        );

        let names = config
            .active_ssh_tunnels()
            .into_iter()
            .map(|(name, _)| name)
            .collect::<BTreeSet<_>>();

        assert_eq!(
            names,
            BTreeSet::from(["ollama".to_string(), "public".to_string()])
        );
    }
}
