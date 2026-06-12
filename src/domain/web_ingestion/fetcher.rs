use async_trait::async_trait;

use super::error::WebIngestionError;

#[derive(Debug, Clone)]
pub struct FetchResult {
    pub final_url: String,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub body_text: String,
    pub content_length: Option<u64>,
}

#[async_trait]
pub trait WebContentFetcher: Send + Sync {
    async fn fetch(
        &self,
        url: &str,
        allowed_domains: Option<&[String]>,
    ) -> Result<FetchResult, WebIngestionError>;
}
