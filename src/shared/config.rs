use serde::Deserialize;

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
    #[serde(default = "default_access_ttl", alias = "expiration_secs")]
    pub access_ttl_secs: u64,
    #[serde(default = "default_refresh_ttl")]
    pub refresh_ttl_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_max_login_attempts")]
    pub max_login_attempts: u32,
    #[serde(default = "default_lockout_duration")]
    pub lockout_duration_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_storage_backend")]
    pub backend: String,
    #[serde(default = "default_storage_base_path")]
    pub base_path: String,
    #[serde(default = "default_storage_base_url")]
    pub base_url: String,
    #[serde(default = "default_max_avatar_bytes")]
    pub max_avatar_bytes: u64,
    #[serde(default = "default_max_image_bytes")]
    pub max_image_bytes: u64,
    #[serde(default = "default_max_audio_bytes")]
    pub max_audio_bytes: u64,
    #[serde(default = "default_max_document_bytes")]
    pub max_document_bytes: u64,
    #[serde(default = "default_max_video_bytes")]
    pub max_video_bytes: u64,
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
pub struct SessionConfig {
    #[serde(default = "default_session_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_secs: u64,
    #[serde(default)]
    pub cleanup_interval_ms: Option<u64>,
}

impl SessionConfig {
    pub fn cleanup_interval_seconds(&self) -> u64 {
        self.cleanup_interval_ms
            .map(|ms| (ms / 1000).max(1))
            .unwrap_or(self.cleanup_interval_secs)
    }
}

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

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PluginsConfig {
    #[serde(default)]
    pub weather: WeatherPluginConfig,
    #[serde(default)]
    pub news: NewsPluginConfig,
    #[serde(default)]
    pub web_search: WebSearchPluginConfig,
    #[serde(default)]
    pub fetch_web_content: FetchWebContentPluginConfig,
    #[serde(default)]
    pub baidu_baike: BaiduBaikePluginConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WeatherPluginConfig {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub default_location: String,
    #[serde(default = "default_weather_city_lookup_endpoint")]
    pub city_lookup_endpoint: String,
    #[serde(default = "default_weather_now_endpoint")]
    pub weather_now_endpoint: String,
    #[serde(default)]
    pub lang_query_enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewsPluginConfig {
    #[serde(default = "default_news_default_rss_url")]
    pub default_rss_url: String,
    #[serde(default = "default_news_society_url")]
    pub society_url: String,
    #[serde(default = "default_news_world_url")]
    pub world_url: String,
    #[serde(default = "default_news_finance_url")]
    pub finance_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebSearchPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_web_search_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FetchWebContentPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub proxy_url: String,
}

impl Default for FetchWebContentPluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            proxy_url: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BaiduBaikePluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub proxy_url: String,
}

impl Default for BaiduBaikePluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            proxy_url: String::new(),
        }
    }
}

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

#[derive(Debug, Clone, Deserialize)]
pub struct CorsConfig {
    #[serde(default = "default_allowed_origins")]
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
}

// ── New config sections ──

#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_llm_provider")]
    pub provider: String,
    #[serde(default = "default_llm_base_url")]
    pub base_url: String,
    #[serde(default = "default_llm_chat_model")]
    pub chat_model: String,
    #[serde(default = "default_llm_embedding_model")]
    pub embedding_model: String,
    #[serde(default = "default_llm_temperature")]
    pub temperature: f64,
    #[serde(default = "default_llm_top_p")]
    pub top_p: f64,
    #[serde(default = "default_llm_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_llm_max_tool_depth")]
    pub max_tool_depth: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_agent_enabled")]
    pub enabled: bool,
    #[serde(default = "default_agent_memory_enabled")]
    pub memory_enabled: bool,
    #[serde(default = "default_agent_rag_enabled")]
    pub rag_enabled: bool,
    #[serde(default = "default_agent_summary_enabled")]
    pub summary_enabled: bool,
    #[serde(default = "default_agent_max_context_messages")]
    pub max_context_messages: u32,
    #[serde(default = "default_agent_max_memory_items")]
    pub max_memory_items: u32,
    #[serde(default = "default_agent_max_rag_chunks")]
    pub max_rag_chunks: u32,
    #[serde(default = "default_agent_memory_extraction_async")]
    pub memory_extraction_async: bool,
    #[serde(default = "default_agent_summary_async")]
    pub summary_async: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RagConfig {
    #[serde(default = "default_rag_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_rag_chunk_overlap")]
    pub chunk_overlap: usize,
    #[serde(default = "default_rag_top_k")]
    pub top_k: usize,
    #[serde(default = "default_rag_hybrid_vector_weight")]
    pub hybrid_vector_weight: f64,
    #[serde(default = "default_rag_hybrid_keyword_weight")]
    pub hybrid_keyword_weight: f64,
}

// ── Default impls ──

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}
impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: default_db_url(),
            max_connections: default_db_max_conn(),
        }
    }
}
impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            secret: default_jwt_secret(),
            access_ttl_secs: default_access_ttl(),
            refresh_ttl_secs: default_refresh_ttl(),
        }
    }
}
impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            max_login_attempts: default_max_login_attempts(),
            lockout_duration_secs: default_lockout_duration(),
        }
    }
}
impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: default_storage_backend(),
            base_path: default_storage_base_path(),
            base_url: default_storage_base_url(),
            max_avatar_bytes: default_max_avatar_bytes(),
            max_image_bytes: default_max_image_bytes(),
            max_audio_bytes: default_max_audio_bytes(),
            max_document_bytes: default_max_document_bytes(),
            max_video_bytes: default_max_video_bytes(),
        }
    }
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
impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: default_session_timeout(),
            cleanup_interval_secs: default_cleanup_interval(),
            cleanup_interval_ms: None,
        }
    }
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
impl Default for WeatherPluginConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            default_location: String::new(),
            city_lookup_endpoint: default_weather_city_lookup_endpoint(),
            weather_now_endpoint: default_weather_now_endpoint(),
            lang_query_enabled: false,
        }
    }
}
impl Default for NewsPluginConfig {
    fn default() -> Self {
        Self {
            default_rss_url: default_news_default_rss_url(),
            society_url: default_news_society_url(),
            world_url: default_news_world_url(),
            finance_url: default_news_finance_url(),
        }
    }
}
impl Default for WebSearchPluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_secs: default_web_search_timeout(),
        }
    }
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
impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: default_allowed_origins(),
        }
    }
}
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
}
impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_llm_provider(),
            base_url: default_llm_base_url(),
            chat_model: default_llm_chat_model(),
            embedding_model: default_llm_embedding_model(),
            temperature: default_llm_temperature(),
            top_p: default_llm_top_p(),
            timeout_secs: default_llm_timeout_secs(),
            max_tool_depth: default_llm_max_tool_depth(),
        }
    }
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: default_agent_enabled(),
            memory_enabled: default_agent_memory_enabled(),
            rag_enabled: default_agent_rag_enabled(),
            summary_enabled: default_agent_summary_enabled(),
            max_context_messages: default_agent_max_context_messages(),
            max_memory_items: default_agent_max_memory_items(),
            max_rag_chunks: default_agent_max_rag_chunks(),
            memory_extraction_async: default_agent_memory_extraction_async(),
            summary_async: default_agent_summary_async(),
        }
    }
}
// ── EmbeddingConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    #[serde(default = "default_embedding_provider")]
    pub provider: String,
    #[serde(default = "default_embedding_base_url")]
    pub base_url: String,
    #[serde(default = "default_embedding_model")]
    pub model: String,
    #[serde(default = "default_embedding_api_key")]
    pub api_key: String,
    #[serde(default = "default_embedding_dimension")]
    pub dimension: usize,
    #[serde(default = "default_embedding_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_embedding_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_qdrant_rag_collection")]
    pub qdrant_collection: String,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: default_embedding_provider(),
            base_url: default_embedding_base_url(),
            model: default_embedding_model(),
            api_key: default_embedding_api_key(),
            dimension: default_embedding_dimension(),
            batch_size: default_embedding_batch_size(),
            timeout_secs: default_embedding_timeout_secs(),
            qdrant_collection: default_qdrant_rag_collection(),
        }
    }
}

// ── WebIngestionConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct WebIngestionConfig {
    #[serde(default = "default_web_ingestion_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub scheduler_enabled: bool,
    #[serde(default)]
    pub dispatcher_enabled: bool,
    /// Global master switch for auto-publish. Default false. When false, the
    /// quality gate never returns Publishable — everything stops at staged and
    /// requires a manual publish request. §5.1 / §15.
    #[serde(default)]
    pub auto_publish: bool,
    #[serde(default = "default_true")]
    pub staging_required: bool,
    #[serde(default = "default_web_ingestion_auto_publish_min_score")]
    pub auto_publish_min_score: f64,
    #[serde(default = "default_web_ingestion_pipeline_version")]
    pub pipeline_version: String,
    #[serde(default = "default_web_ingestion_llm_prompt_version")]
    pub llm_prompt_version: String,
    #[serde(default = "default_web_ingestion_chunker_version")]
    pub chunker_version: String,
    #[serde(default = "default_web_ingestion_embedding_batch_size")]
    pub embedding_batch_size: usize,
    #[serde(default = "default_web_ingestion_qdrant_collection")]
    pub qdrant_collection: String,
    #[serde(default = "default_web_ingestion_max_body_bytes")]
    pub max_body_bytes: u64,
    #[serde(default = "default_web_ingestion_fetch_timeout_secs")]
    pub fetch_timeout_secs: u64,
    #[serde(default = "default_web_ingestion_fetch_user_agent")]
    pub fetch_user_agent: String,
    #[serde(default)]
    pub fetch_proxy_url: String,
    #[serde(default = "default_web_ingestion_min_request_interval_ms")]
    pub min_request_interval_ms: u64,
    #[serde(default = "default_web_ingestion_request_jitter_ms")]
    pub request_jitter_ms: u64,
    #[serde(default = "default_web_ingestion_max_urls_per_source_per_job")]
    pub max_urls_per_source_per_job: usize,
    #[serde(default = "default_web_ingestion_url_enqueue_dedupe_secs")]
    pub url_enqueue_dedupe_secs: u64,
    #[serde(default = "default_web_ingestion_chunk_target_min")]
    pub chunk_target_min: usize,
    #[serde(default = "default_web_ingestion_chunk_target_max")]
    pub chunk_target_max: usize,
    #[serde(default = "default_web_ingestion_chunk_overlap_min")]
    pub chunk_overlap_min: usize,
    #[serde(default = "default_web_ingestion_chunk_overlap_max")]
    pub chunk_overlap_max: usize,
    #[serde(default = "default_web_ingestion_scheduler_interval_secs")]
    pub scheduler_interval_secs: u64,
    #[serde(default = "default_web_ingestion_dispatcher_interval_secs")]
    pub dispatcher_interval_secs: u64,
    #[serde(default = "default_web_ingestion_outbox_batch_size")]
    pub outbox_batch_size: u64,
    #[serde(default = "default_web_ingestion_outbox_lock_ttl_secs")]
    pub outbox_lock_ttl_secs: u32,
    #[serde(default = "default_web_ingestion_retry_base_delay_secs")]
    pub retry_base_delay_secs: u64,
    #[serde(default = "default_web_ingestion_retry_max_delay_secs")]
    pub retry_max_delay_secs: u64,
    #[serde(default)]
    pub distill_llm: DistillLlmConfig,
}

impl Default for WebIngestionConfig {
    fn default() -> Self {
        Self {
            enabled: default_web_ingestion_enabled(),
            scheduler_enabled: false,
            dispatcher_enabled: false,
            auto_publish: false,
            staging_required: true,
            auto_publish_min_score: default_web_ingestion_auto_publish_min_score(),
            pipeline_version: default_web_ingestion_pipeline_version(),
            llm_prompt_version: default_web_ingestion_llm_prompt_version(),
            chunker_version: default_web_ingestion_chunker_version(),
            embedding_batch_size: default_web_ingestion_embedding_batch_size(),
            qdrant_collection: default_web_ingestion_qdrant_collection(),
            max_body_bytes: default_web_ingestion_max_body_bytes(),
            fetch_timeout_secs: default_web_ingestion_fetch_timeout_secs(),
            fetch_user_agent: default_web_ingestion_fetch_user_agent(),
            fetch_proxy_url: String::new(),
            min_request_interval_ms: default_web_ingestion_min_request_interval_ms(),
            request_jitter_ms: default_web_ingestion_request_jitter_ms(),
            max_urls_per_source_per_job: default_web_ingestion_max_urls_per_source_per_job(),
            url_enqueue_dedupe_secs: default_web_ingestion_url_enqueue_dedupe_secs(),
            chunk_target_min: default_web_ingestion_chunk_target_min(),
            chunk_target_max: default_web_ingestion_chunk_target_max(),
            chunk_overlap_min: default_web_ingestion_chunk_overlap_min(),
            chunk_overlap_max: default_web_ingestion_chunk_overlap_max(),
            scheduler_interval_secs: default_web_ingestion_scheduler_interval_secs(),
            dispatcher_interval_secs: default_web_ingestion_dispatcher_interval_secs(),
            outbox_batch_size: default_web_ingestion_outbox_batch_size(),
            outbox_lock_ttl_secs: default_web_ingestion_outbox_lock_ttl_secs(),
            retry_base_delay_secs: default_web_ingestion_retry_base_delay_secs(),
            retry_max_delay_secs: default_web_ingestion_retry_max_delay_secs(),
            distill_llm: DistillLlmConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DistillLlmConfig {
    #[serde(default = "default_distill_llm_provider")]
    pub provider: String,
    #[serde(default = "default_distill_llm_base_url")]
    pub base_url: String,
    #[serde(default = "default_distill_llm_chat_model")]
    pub chat_model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_distill_llm_temperature")]
    pub temperature: f64,
    #[serde(default = "default_distill_llm_top_p")]
    pub top_p: f64,
    #[serde(default = "default_distill_llm_timeout_secs")]
    pub timeout_secs: u64,
}

impl Default for DistillLlmConfig {
    fn default() -> Self {
        Self {
            provider: default_distill_llm_provider(),
            base_url: default_distill_llm_base_url(),
            chat_model: default_distill_llm_chat_model(),
            api_key: String::new(),
            temperature: default_distill_llm_temperature(),
            top_p: default_distill_llm_top_p(),
            timeout_secs: default_distill_llm_timeout_secs(),
        }
    }
}

fn default_embedding_provider() -> String {
    "ollama".into()
}
fn default_embedding_base_url() -> String {
    "http://127.0.0.1:11434".into()
}
fn default_embedding_model() -> String {
    "nomic-embed-text".into()
}
fn default_embedding_api_key() -> String {
    String::new()
}
fn default_embedding_dimension() -> usize {
    768
}
fn default_embedding_batch_size() -> usize {
    32
}
fn default_embedding_timeout_secs() -> u64 {
    120
}

fn default_web_ingestion_enabled() -> bool {
    false
}
fn default_web_ingestion_auto_publish_min_score() -> f64 {
    0.85
}
fn default_web_ingestion_pipeline_version() -> String {
    "20260612".into()
}
fn default_web_ingestion_llm_prompt_version() -> String {
    "20260612_v1".into()
}
fn default_web_ingestion_chunker_version() -> String {
    "20260612".into()
}
fn default_web_ingestion_embedding_batch_size() -> usize {
    32
}
fn default_web_ingestion_qdrant_collection() -> String {
    "web_ingestion".into()
}
fn default_web_ingestion_max_body_bytes() -> u64 {
    5 * 1024 * 1024
}
fn default_web_ingestion_fetch_timeout_secs() -> u64 {
    30
}
fn default_web_ingestion_fetch_user_agent() -> String {
    "ServerRSKnowledgeBot/0.1".into()
}
fn default_web_ingestion_min_request_interval_ms() -> u64 {
    2_000
}
fn default_web_ingestion_request_jitter_ms() -> u64 {
    1_000
}
fn default_web_ingestion_max_urls_per_source_per_job() -> usize {
    20
}
fn default_web_ingestion_url_enqueue_dedupe_secs() -> u64 {
    86_400
}
fn default_web_ingestion_chunk_target_min() -> usize {
    500
}
fn default_web_ingestion_chunk_target_max() -> usize {
    1000
}
fn default_web_ingestion_chunk_overlap_min() -> usize {
    80
}
fn default_web_ingestion_chunk_overlap_max() -> usize {
    120
}
fn default_web_ingestion_scheduler_interval_secs() -> u64 {
    300
}
fn default_web_ingestion_dispatcher_interval_secs() -> u64 {
    5
}
fn default_web_ingestion_outbox_batch_size() -> u64 {
    20
}
fn default_web_ingestion_outbox_lock_ttl_secs() -> u32 {
    300
}
fn default_web_ingestion_retry_base_delay_secs() -> u64 {
    30
}
fn default_web_ingestion_retry_max_delay_secs() -> u64 {
    1800
}

fn default_distill_llm_provider() -> String {
    "deepseek".into()
}
fn default_distill_llm_base_url() -> String {
    "https://api.deepseek.com".into()
}
fn default_distill_llm_chat_model() -> String {
    "deepseek-chat".into()
}
fn default_distill_llm_temperature() -> f64 {
    0.1
}
fn default_distill_llm_top_p() -> f64 {
    0.9
}
fn default_distill_llm_timeout_secs() -> u64 {
    120
}

// ── QdrantConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct QdrantConfig {
    #[serde(default = "default_qdrant_enabled")]
    pub enabled: bool,
    #[serde(default = "default_qdrant_url")]
    pub url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_qdrant_rag_collection")]
    pub rag_collection: String,
    #[serde(default = "default_qdrant_memory_collection")]
    pub memory_collection: String,
    #[serde(default = "default_qdrant_summary_collection")]
    pub summary_collection: String,
}

impl Default for QdrantConfig {
    fn default() -> Self {
        Self {
            enabled: default_qdrant_enabled(),
            url: default_qdrant_url(),
            api_key: None,
            rag_collection: default_qdrant_rag_collection(),
            memory_collection: default_qdrant_memory_collection(),
            summary_collection: default_qdrant_summary_collection(),
        }
    }
}

fn default_qdrant_enabled() -> bool {
    false
}
fn default_qdrant_url() -> String {
    "http://127.0.0.1:6333".into()
}
fn default_qdrant_rag_collection() -> String {
    "rag_chunks".into()
}
fn default_qdrant_memory_collection() -> String {
    "user_memories".into()
}
fn default_qdrant_summary_collection() -> String {
    "conversation_summaries".into()
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            chunk_size: default_rag_chunk_size(),
            chunk_overlap: default_rag_chunk_overlap(),
            top_k: default_rag_top_k(),
            hybrid_vector_weight: default_rag_hybrid_vector_weight(),
            hybrid_keyword_weight: default_rag_hybrid_keyword_weight(),
        }
    }
}

// ── Default fns ──

fn default_host() -> String {
    "0.0.0.0".into()
}
fn default_port() -> u16 {
    8080
}
fn default_db_url() -> String {
    "mysql://user:password@127.0.0.1:3306/app_db".into()
}
fn default_db_max_conn() -> u32 {
    10
}
fn default_jwt_secret() -> String {
    "CHANGE_ME_USE_A_LONG_RANDOM_SECRET".into()
}
fn default_access_ttl() -> u64 {
    900
}
fn default_refresh_ttl() -> u64 {
    2_592_000
}
fn default_max_login_attempts() -> u32 {
    5
}
fn default_lockout_duration() -> u64 {
    900
}
fn default_storage_backend() -> String {
    "LOCAL".into()
}
fn default_storage_base_path() -> String {
    "./uploads".into()
}
fn default_storage_base_url() -> String {
    "http://localhost:8080/files".into()
}
fn default_max_avatar_bytes() -> u64 {
    2 * 1024 * 1024
}
fn default_max_image_bytes() -> u64 {
    10 * 1024 * 1024
}
fn default_max_audio_bytes() -> u64 {
    50 * 1024 * 1024
}
fn default_max_document_bytes() -> u64 {
    20 * 1024 * 1024
}
fn default_max_video_bytes() -> u64 {
    200 * 1024 * 1024
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
fn default_session_timeout() -> u64 {
    1800
}
fn default_cleanup_interval() -> u64 {
    300
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
fn default_weather_city_lookup_endpoint() -> String {
    "https://mk4ky3n4am.re.qweatherapi.com/geo/v2/city/lookup".into()
}
fn default_weather_now_endpoint() -> String {
    "https://mk4ky3n4am.re.qweatherapi.com/v7/weather/now".into()
}
fn default_news_default_rss_url() -> String {
    "https://www.chinanews.com.cn/rss/society.xml".into()
}
fn default_news_society_url() -> String {
    "https://www.chinanews.com.cn/rss/society.xml".into()
}
fn default_news_world_url() -> String {
    "https://www.chinanews.com.cn/rss/world.xml".into()
}
fn default_news_finance_url() -> String {
    "https://www.chinanews.com.cn/rss/finance.xml".into()
}
fn default_web_search_timeout() -> u64 {
    10
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
fn default_allowed_origins() -> Vec<String> {
    vec!["http://localhost:3000".into()]
}
fn default_log_level() -> String {
    "info".into()
}
fn default_true() -> bool {
    true
}

// ── LlmConfig defaults ──

fn default_llm_provider() -> String {
    "openai".into()
}
fn default_llm_base_url() -> String {
    "http://127.0.0.1:11434/v1".into()
}
fn default_llm_chat_model() -> String {
    "qwen2.5:14b".into()
}
fn default_llm_embedding_model() -> String {
    "bge-m3".into()
}
fn default_llm_temperature() -> f64 {
    0.7
}
fn default_llm_top_p() -> f64 {
    0.9
}
fn default_llm_timeout_secs() -> u64 {
    120
}
fn default_llm_max_tool_depth() -> u32 {
    10
}

// ── AgentConfig defaults ──

fn default_agent_enabled() -> bool {
    false
}
fn default_agent_memory_enabled() -> bool {
    true
}
fn default_agent_rag_enabled() -> bool {
    true
}
fn default_agent_summary_enabled() -> bool {
    true
}
fn default_agent_max_context_messages() -> u32 {
    50
}
fn default_agent_max_memory_items() -> u32 {
    100
}
fn default_agent_max_rag_chunks() -> u32 {
    5
}
fn default_agent_memory_extraction_async() -> bool {
    true
}
fn default_agent_summary_async() -> bool {
    true
}

// ── RagConfig defaults ──

fn default_rag_chunk_size() -> usize {
    512
}
fn default_rag_chunk_overlap() -> usize {
    64
}
fn default_rag_top_k() -> usize {
    5
}
fn default_rag_hybrid_vector_weight() -> f64 {
    0.7
}
fn default_rag_hybrid_keyword_weight() -> f64 {
    0.3
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
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        // Load .env file into process environment BEFORE reading config.
        // dotenvy::dotenv() is a no-op if .env doesn't exist, so it's safe
        // in production (where env vars are set via the orchestrator).
        let _ = dotenvy::dotenv();

        let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".into());
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(cfg) => {
                    tracing::info!(path = %path, "configuration loaded");
                    cfg
                }
                Err(e) => panic!("failed to parse configuration file {path}: {e}"),
            },
            Err(e) => {
                tracing::warn!(path = %path, error = %e, "config file not found, using defaults");
                Self::default()
            }
        };
        cfg.apply_env_overrides();
        cfg.validate()
            .unwrap_or_else(|e| panic!("invalid application configuration: {e}"));
        cfg
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.database.url.trim().is_empty() {
            return Err("database.url cannot be empty".into());
        }
        if self.database.max_connections == 0 {
            return Err("database.max_connections must be at least 1".into());
        }
        if self.jwt.secret == default_jwt_secret() || self.jwt.secret.len() < 32 {
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
            // DEEPSEEK_API_KEY is a fallback for distill_llm ONLY, never for embedding
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
        // ── Embedding (separate from distill_llm — DEEPSEEK_API_KEY must NOT leak here) ──
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
    }
}
