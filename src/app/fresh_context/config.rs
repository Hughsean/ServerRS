//! Fresh Context 用例配置。
//!
//! 此类型只包含采集、蒸馏、排序和索引策略所需的业务参数；HTTP、模型连接
//! 与向量存储位置由启动层通过基础设施适配器配置处理。

use crate::shared::config::FreshContextConfig;

#[derive(Debug, Clone)]
pub struct FreshContextUseCaseConfig {
    pub max_sources_per_tick: usize,
    pub max_items_per_source: usize,
    pub max_pipeline_items_per_tick: usize,
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub max_indexable_chunks_per_tick: usize,
    pub max_topic_items_per_tick: usize,
    pub max_expired_vectors_per_tick: usize,
    pub max_retrieval_chunks: usize,
    pub trend_ttl_secs: u64,
    pub gossip_ttl_secs: u64,
    pub news_ttl_secs: u64,
    pub background_ttl_secs: u64,
    pub min_reliability_score: f64,
    pub semantic_weight: f64,
    pub freshness_weight: f64,
    pub reliability_weight: f64,
    pub heat_weight: f64,
}

impl From<&FreshContextConfig> for FreshContextUseCaseConfig {
    fn from(config: &FreshContextConfig) -> Self {
        Self {
            max_sources_per_tick: config.max_sources_per_tick,
            max_items_per_source: config.max_items_per_source,
            max_pipeline_items_per_tick: config.max_pipeline_items_per_tick,
            chunk_size: config.chunk_size,
            chunk_overlap: config.chunk_overlap,
            max_indexable_chunks_per_tick: config.max_indexable_chunks_per_tick,
            max_topic_items_per_tick: config.max_topic_items_per_tick,
            max_expired_vectors_per_tick: config.max_expired_vectors_per_tick,
            max_retrieval_chunks: config.max_retrieval_chunks,
            trend_ttl_secs: config.trend_ttl_secs,
            gossip_ttl_secs: config.gossip_ttl_secs,
            news_ttl_secs: config.news_ttl_secs,
            background_ttl_secs: config.background_ttl_secs,
            min_reliability_score: config.min_reliability_score,
            semantic_weight: config.semantic_weight,
            freshness_weight: config.freshness_weight,
            reliability_weight: config.reliability_weight,
            heat_weight: config.heat_weight,
        }
    }
}

impl Default for FreshContextUseCaseConfig {
    fn default() -> Self {
        Self::from(&FreshContextConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::FreshContextUseCaseConfig;
    use crate::shared::config::FreshContextConfig;

    #[test]
    fn excludes_adapter_details_when_mapping_shared_configuration() {
        let config = FreshContextConfig {
            vector_index_name: "fresh-test".into(),
            fetch_timeout_secs: 5,
            ..FreshContextConfig::default()
        };

        let use_case = FreshContextUseCaseConfig::from(&config);

        assert_eq!(use_case.max_retrieval_chunks, config.max_retrieval_chunks);
        assert_eq!(use_case.chunk_size, config.chunk_size);
    }
}
