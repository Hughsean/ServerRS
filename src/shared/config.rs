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
    #[serde(default = "default_access_ttl")]
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
    #[serde(default = "default_weather_provider")]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewsPluginConfig {
    #[serde(default = "default_news_rss_url")]
    pub rss_url: String,
    #[serde(default)]
    pub category_urls: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebSearchPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_web_search_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FetchWebContentPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BaiduBaikePluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
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
            provider: default_weather_provider(),
            api_key: String::new(),
        }
    }
}
impl Default for NewsPluginConfig {
    fn default() -> Self {
        Self {
            rss_url: default_news_rss_url(),
            category_urls: Default::default(),
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
    #[serde(default = "default_embedding_dimension")]
    pub dimension: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: default_embedding_provider(),
            base_url: default_embedding_base_url(),
            model: default_embedding_model(),
            dimension: default_embedding_dimension(),
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
fn default_embedding_dimension() -> usize {
    768
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
fn default_weather_provider() -> String {
    "openweathermap".into()
}
fn default_news_rss_url() -> String {
    "https://feeds.bbci.co.uk/news/rss.xml".into()
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
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".into());
        match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(cfg) => {
                    tracing::info!(path = %path, "configuration loaded");
                    cfg
                }
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "failed to parse config, using defaults");
                    Self::default()
                }
            },
            Err(e) => {
                tracing::warn!(path = %path, error = %e, "config file not found, using defaults");
                Self::default()
            }
        }
    }
}
