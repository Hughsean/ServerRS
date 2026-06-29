use serde::Deserialize;

use super::DistillLlmConfig;

#[derive(Debug, Clone, Deserialize)]
pub struct FreshContextConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub scheduler_enabled: bool,
    #[serde(default)]
    pub dispatcher_enabled: bool,
    #[serde(default = "default_collection")]
    pub qdrant_collection: String,
    #[serde(default = "default_scheduler_interval_secs")]
    pub scheduler_interval_secs: u64,
    #[serde(default = "default_dispatcher_interval_secs")]
    pub dispatcher_interval_secs: u64,
    #[serde(default = "default_max_sources_per_tick")]
    pub max_sources_per_tick: usize,
    #[serde(default = "default_max_items_per_source")]
    pub max_items_per_source: usize,
    #[serde(default = "default_max_pipeline_items_per_tick")]
    pub max_pipeline_items_per_tick: usize,
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: usize,
    #[serde(default = "default_max_indexable_chunks_per_tick")]
    pub max_indexable_chunks_per_tick: usize,
    #[serde(default = "default_max_topic_items_per_tick")]
    pub max_topic_items_per_tick: usize,
    #[serde(default = "default_max_expired_vectors_per_tick")]
    pub max_expired_vectors_per_tick: usize,
    #[serde(default = "default_max_retrieval_chunks")]
    pub max_retrieval_chunks: usize,
    #[serde(default = "default_fetch_timeout_secs")]
    pub fetch_timeout_secs: u64,
    #[serde(default = "default_user_agent")]
    pub fetch_user_agent: String,
    #[serde(default)]
    pub fetch_proxy_url: String,
    #[serde(default = "default_trend_ttl_secs")]
    pub trend_ttl_secs: u64,
    #[serde(default = "default_gossip_ttl_secs")]
    pub gossip_ttl_secs: u64,
    #[serde(default = "default_news_ttl_secs")]
    pub news_ttl_secs: u64,
    #[serde(default = "default_background_ttl_secs")]
    pub background_ttl_secs: u64,
    #[serde(default = "default_min_reliability_score")]
    pub min_reliability_score: f64,
    #[serde(default = "default_semantic_weight")]
    pub semantic_weight: f64,
    #[serde(default = "default_freshness_weight")]
    pub freshness_weight: f64,
    #[serde(default = "default_reliability_weight")]
    pub reliability_weight: f64,
    #[serde(default = "default_heat_weight")]
    pub heat_weight: f64,
    #[serde(default)]
    pub distill_llm: DistillLlmConfig,
}

impl Default for FreshContextConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scheduler_enabled: false,
            dispatcher_enabled: false,
            qdrant_collection: default_collection(),
            scheduler_interval_secs: default_scheduler_interval_secs(),
            dispatcher_interval_secs: default_dispatcher_interval_secs(),
            max_sources_per_tick: default_max_sources_per_tick(),
            max_items_per_source: default_max_items_per_source(),
            max_pipeline_items_per_tick: default_max_pipeline_items_per_tick(),
            chunk_size: default_chunk_size(),
            chunk_overlap: default_chunk_overlap(),
            max_indexable_chunks_per_tick: default_max_indexable_chunks_per_tick(),
            max_topic_items_per_tick: default_max_topic_items_per_tick(),
            max_expired_vectors_per_tick: default_max_expired_vectors_per_tick(),
            max_retrieval_chunks: default_max_retrieval_chunks(),
            fetch_timeout_secs: default_fetch_timeout_secs(),
            fetch_user_agent: default_user_agent(),
            fetch_proxy_url: String::new(),
            trend_ttl_secs: default_trend_ttl_secs(),
            gossip_ttl_secs: default_gossip_ttl_secs(),
            news_ttl_secs: default_news_ttl_secs(),
            background_ttl_secs: default_background_ttl_secs(),
            min_reliability_score: default_min_reliability_score(),
            semantic_weight: default_semantic_weight(),
            freshness_weight: default_freshness_weight(),
            reliability_weight: default_reliability_weight(),
            heat_weight: default_heat_weight(),
            distill_llm: DistillLlmConfig::default(),
        }
    }
}

fn default_collection() -> String {
    "fresh_chunks_2560".into()
}
fn default_scheduler_interval_secs() -> u64 {
    900
}
fn default_dispatcher_interval_secs() -> u64 {
    30
}
fn default_max_sources_per_tick() -> usize {
    8
}
fn default_max_items_per_source() -> usize {
    50
}
fn default_max_pipeline_items_per_tick() -> usize {
    50
}
fn default_chunk_size() -> usize {
    900
}
fn default_chunk_overlap() -> usize {
    120
}
fn default_max_indexable_chunks_per_tick() -> usize {
    100
}
fn default_max_topic_items_per_tick() -> usize {
    100
}
fn default_max_expired_vectors_per_tick() -> usize {
    200
}
fn default_max_retrieval_chunks() -> usize {
    3
}
fn default_fetch_timeout_secs() -> u64 {
    20
}
fn default_user_agent() -> String {
    "ServerRSFreshBot/0.1".into()
}
fn default_trend_ttl_secs() -> u64 {
    24 * 60 * 60
}
fn default_gossip_ttl_secs() -> u64 {
    3 * 24 * 60 * 60
}
fn default_news_ttl_secs() -> u64 {
    14 * 24 * 60 * 60
}
fn default_background_ttl_secs() -> u64 {
    180 * 24 * 60 * 60
}
fn default_min_reliability_score() -> f64 {
    0.35
}
fn default_semantic_weight() -> f64 {
    0.55
}
fn default_freshness_weight() -> f64 {
    0.25
}
fn default_reliability_weight() -> f64 {
    0.15
}
fn default_heat_weight() -> f64 {
    0.05
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_collection_matches_example_config() {
        let config = FreshContextConfig::default();
        assert_eq!(config.qdrant_collection, "fresh_chunks_2560");
        assert_eq!(config.dispatcher_interval_secs, 30);
        assert_eq!(config.max_pipeline_items_per_tick, 50);
        assert_eq!(config.chunk_size, 900);
        assert_eq!(config.chunk_overlap, 120);
        assert_eq!(config.max_indexable_chunks_per_tick, 100);
        assert_eq!(config.max_topic_items_per_tick, 100);
        assert_eq!(config.max_expired_vectors_per_tick, 200);
        assert_eq!(config.max_retrieval_chunks, 3);
    }

    #[test]
    fn parses_nested_distill_llm_config() {
        let raw = r#"
            [fresh_context]
            enabled = true
            dispatcher_interval_secs = 15
            max_pipeline_items_per_tick = 25
            chunk_size = 500
            chunk_overlap = 50
            max_indexable_chunks_per_tick = 75
            max_topic_items_per_tick = 80
            max_expired_vectors_per_tick = 90
            max_retrieval_chunks = 2

            [fresh_context.distill_llm]
            provider = "ollama"
            base_url = "http://127.0.0.1:11434/v1"
            chat_model = "qwen3:14b"
            timeout_secs = 180
        "#;

        let config: crate::shared::config::AppConfig = toml::from_str(raw).unwrap();
        assert!(config.fresh_context.enabled);
        assert_eq!(config.fresh_context.dispatcher_interval_secs, 15);
        assert_eq!(config.fresh_context.max_pipeline_items_per_tick, 25);
        assert_eq!(config.fresh_context.chunk_size, 500);
        assert_eq!(config.fresh_context.chunk_overlap, 50);
        assert_eq!(config.fresh_context.max_indexable_chunks_per_tick, 75);
        assert_eq!(config.fresh_context.max_topic_items_per_tick, 80);
        assert_eq!(config.fresh_context.max_expired_vectors_per_tick, 90);
        assert_eq!(config.fresh_context.max_retrieval_chunks, 2);
        assert_eq!(config.fresh_context.distill_llm.provider, "ollama");
        assert_eq!(config.fresh_context.distill_llm.timeout_secs, 180);
    }
}
