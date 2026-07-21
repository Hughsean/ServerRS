//! Fresh Context 基础设施适配器配置。

use crate::shared::config::{DistillLlmConfig, FreshContextConfig};

#[derive(Debug, Clone)]
pub struct FreshContextAdapterConfig {
    pub vector_index_name: String,
    pub fetch_timeout_secs: u64,
    pub fetch_user_agent: String,
    pub fetch_proxy_url: String,
    pub distill_llm: DistillLlmConfig,
}

impl From<&FreshContextConfig> for FreshContextAdapterConfig {
    fn from(config: &FreshContextConfig) -> Self {
        Self {
            vector_index_name: config.vector_index_name.clone(),
            fetch_timeout_secs: config.fetch_timeout_secs,
            fetch_user_agent: config.fetch_user_agent.clone(),
            fetch_proxy_url: config.fetch_proxy_url.clone(),
            distill_llm: config.distill_llm.clone(),
        }
    }
}
