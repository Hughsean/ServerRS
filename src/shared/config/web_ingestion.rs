use serde::Deserialize;

use super::default_true;

// ── DistillLlmConfig ──

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

// ── WebIngestionConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct WebIngestionConfig {
    #[serde(default = "default_web_ingestion_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub scheduler_enabled: bool,
    #[serde(default)]
    pub dispatcher_enabled: bool,
    /// 自动发布的全局主开关。默认为 false。 When false, the
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
    #[serde(default = "default_web_ingestion_dispatcher_parallelism")]
    pub dispatcher_parallelism: usize,
    #[serde(default = "default_web_ingestion_outbox_lock_ttl_secs")]
    pub outbox_lock_ttl_secs: u32,
    #[serde(default = "default_web_ingestion_retry_base_delay_secs")]
    pub retry_base_delay_secs: u64,
    #[serde(default = "default_web_ingestion_retry_max_delay_secs")]
    pub retry_max_delay_secs: u64,
    #[serde(default)]
    pub handler_parallelism: WebIngestionHandlerParallelismConfig,
    #[serde(default)]
    pub distill_llm: DistillLlmConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebIngestionHandlerParallelismConfig {
    #[serde(default = "default_web_ingestion_handler_default_parallelism")]
    pub default: usize,
    #[serde(default = "default_web_ingestion_handler_crawl_job_created_parallelism")]
    pub crawl_job_created: usize,
    #[serde(default = "default_web_ingestion_handler_url_discovered_parallelism")]
    pub url_discovered: usize,
    #[serde(default = "default_web_ingestion_handler_page_fetched_parallelism")]
    pub page_fetched: usize,
    #[serde(default = "default_web_ingestion_handler_page_cleaned_parallelism")]
    pub page_cleaned: usize,
    #[serde(default = "default_web_ingestion_handler_page_distilled_parallelism")]
    pub page_distilled: usize,
    #[serde(default = "default_web_ingestion_handler_quality_checked_parallelism")]
    pub quality_checked: usize,
    #[serde(default = "default_web_ingestion_handler_document_chunked_parallelism")]
    pub document_chunked: usize,
    #[serde(default = "default_web_ingestion_handler_chunks_embedded_parallelism")]
    pub chunks_embedded: usize,
    #[serde(default = "default_web_ingestion_handler_document_indexed_parallelism")]
    pub document_indexed: usize,
    #[serde(default = "default_web_ingestion_handler_knowledge_staged_parallelism")]
    pub knowledge_staged: usize,
    #[serde(default = "default_web_ingestion_handler_publish_requested_parallelism")]
    pub knowledge_publish_requested: usize,
    #[serde(default = "default_web_ingestion_handler_rollback_requested_parallelism")]
    pub knowledge_rollback_requested: usize,
    #[serde(default = "default_web_ingestion_handler_terminal_parallelism")]
    pub terminal: usize,
}

impl Default for WebIngestionHandlerParallelismConfig {
    fn default() -> Self {
        Self {
            default: default_web_ingestion_handler_default_parallelism(),
            crawl_job_created: default_web_ingestion_handler_crawl_job_created_parallelism(),
            url_discovered: default_web_ingestion_handler_url_discovered_parallelism(),
            page_fetched: default_web_ingestion_handler_page_fetched_parallelism(),
            page_cleaned: default_web_ingestion_handler_page_cleaned_parallelism(),
            page_distilled: default_web_ingestion_handler_page_distilled_parallelism(),
            quality_checked: default_web_ingestion_handler_quality_checked_parallelism(),
            document_chunked: default_web_ingestion_handler_document_chunked_parallelism(),
            chunks_embedded: default_web_ingestion_handler_chunks_embedded_parallelism(),
            document_indexed: default_web_ingestion_handler_document_indexed_parallelism(),
            knowledge_staged: default_web_ingestion_handler_knowledge_staged_parallelism(),
            knowledge_publish_requested:
                default_web_ingestion_handler_publish_requested_parallelism(),
            knowledge_rollback_requested:
                default_web_ingestion_handler_rollback_requested_parallelism(),
            terminal: default_web_ingestion_handler_terminal_parallelism(),
        }
    }
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
            dispatcher_parallelism: default_web_ingestion_dispatcher_parallelism(),
            outbox_lock_ttl_secs: default_web_ingestion_outbox_lock_ttl_secs(),
            retry_base_delay_secs: default_web_ingestion_retry_base_delay_secs(),
            retry_max_delay_secs: default_web_ingestion_retry_max_delay_secs(),
            handler_parallelism: WebIngestionHandlerParallelismConfig::default(),
            distill_llm: DistillLlmConfig::default(),
        }
    }
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
fn default_web_ingestion_dispatcher_parallelism() -> usize {
    1
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

fn default_web_ingestion_handler_default_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_crawl_job_created_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_url_discovered_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_page_fetched_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_page_cleaned_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_page_distilled_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_quality_checked_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_document_chunked_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_chunks_embedded_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_document_indexed_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_knowledge_staged_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_publish_requested_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_rollback_requested_parallelism() -> usize {
    1
}
fn default_web_ingestion_handler_terminal_parallelism() -> usize {
    1
}

#[cfg(test)]
mod tests {
    use super::super::AppConfig;

    #[test]
    fn parses_handler_parallelism_with_distill_table_declared_first() {
        let raw = r#"
            [web_ingestion.distill_llm]
            provider = "ollama"
            chat_model = "qwen3:8b"

            [web_ingestion]
            enabled = true
            dispatcher_enabled = true
            dispatcher_parallelism = 10

            [web_ingestion.handler_parallelism]
            url_discovered = 4
            page_cleaned = 2
            chunks_embedded = 2
            terminal = 8
        "#;

        let config: AppConfig = toml::from_str(raw).expect("config should parse");
        assert!(config.web_ingestion.enabled);
        assert_eq!(config.web_ingestion.distill_llm.provider, "ollama");
        assert_eq!(config.web_ingestion.dispatcher_parallelism, 10);
        assert_eq!(config.web_ingestion.handler_parallelism.url_discovered, 4);
        assert_eq!(config.web_ingestion.handler_parallelism.page_cleaned, 2);
        assert_eq!(config.web_ingestion.handler_parallelism.chunks_embedded, 2);
        assert_eq!(config.web_ingestion.handler_parallelism.terminal, 8);
    }
}
