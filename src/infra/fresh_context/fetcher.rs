use async_trait::async_trait;

use crate::domain::fresh_context::{FreshContentFetcher, FreshFetchResult};
use crate::domain::web_ingestion::fetcher::WebContentFetcher;
use crate::infra::web_ingestion::fetcher::WebFetcher;
use crate::shared::config::{FreshContextConfig, WebIngestionConfig};
use crate::shared::error::AppError;

pub struct FreshContextWebFetcher {
    inner: WebFetcher,
}

impl FreshContextWebFetcher {
    pub fn new(config: &FreshContextConfig) -> Result<Self, AppError> {
        let web_config = WebIngestionConfig {
            max_body_bytes: 2 * 1024 * 1024,
            fetch_timeout_secs: config.fetch_timeout_secs,
            fetch_user_agent: config.fetch_user_agent.clone(),
            fetch_proxy_url: config.fetch_proxy_url.clone(),
            min_request_interval_ms: 1_000,
            request_jitter_ms: 500,
            ..WebIngestionConfig::default()
        };
        let inner = WebFetcher::new(&web_config)
            .map_err(|e| AppError::Infrastructure(format!("fresh fetcher init: {e}")))?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl FreshContentFetcher for FreshContextWebFetcher {
    async fn fetch(
        &self,
        url: &str,
        allowed_domains: Option<&[String]>,
    ) -> Result<FreshFetchResult, AppError> {
        let fetched = self
            .inner
            .fetch(url, allowed_domains)
            .await
            .map_err(|e| AppError::Infrastructure(format!("fresh fetch failed: {e}")))?;
        Ok(FreshFetchResult {
            final_url: fetched.final_url,
            content_type: fetched.content_type,
            body_text: fetched.body_text,
            content_length: fetched.content_length,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_fetcher_from_fresh_config() {
        let config = FreshContextConfig {
            fetch_proxy_url: String::new(),
            ..FreshContextConfig::default()
        };
        assert!(FreshContextWebFetcher::new(&config).is_ok());
    }
}
