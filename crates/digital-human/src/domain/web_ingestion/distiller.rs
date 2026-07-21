use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::error::WebIngestionError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistilledDocument {
    pub accept: bool,
    #[serde(default)]
    pub reject_reason: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub sections: Vec<DistilledSection>,
    #[serde(default)]
    pub quality_score: f64,
    #[serde(default)]
    pub risk_flags: Vec<String>,
    #[serde(default)]
    pub freshness_level: String,
    #[serde(default)]
    pub should_publish: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistilledSection {
    #[serde(default)]
    pub heading: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DistillResult {
    pub distilled: DistilledDocument,
    pub llm_input_tokens: Option<u32>,
    pub llm_output_tokens: Option<u32>,
}

#[async_trait]
pub trait KnowledgeDistiller: Send + Sync {
    async fn distill(
        &self,
        cleaned_text: &str,
        url: &str,
    ) -> Result<DistillResult, WebIngestionError>;
}
