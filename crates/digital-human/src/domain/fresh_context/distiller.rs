use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::shared::error::AppError;

use super::{rumor_level, source_kind};

#[derive(Debug, Clone)]
pub struct FreshDistillInput {
    pub source_name: String,
    pub source_kind: String,
    pub trust_level: String,
    pub url: Option<String>,
    pub title: Option<String>,
    pub clean_text: String,
    pub published_at: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshDistilledItem {
    pub accept: bool,
    #[serde(default)]
    pub reject_reason: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub language: String,
    #[serde(default = "default_content_type")]
    pub content_type: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub claims: Vec<FreshDistilledClaim>,
    #[serde(default)]
    pub entities: Vec<FreshDistilledEntity>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub topic_key_hint: String,
    #[serde(default = "default_rumor_level")]
    pub rumor_level: String,
    #[serde(default)]
    pub risk_flags: Vec<String>,
    #[serde(default)]
    pub freshness_score: f64,
    #[serde(default)]
    pub heat_score: f64,
    #[serde(default = "default_ttl_hint")]
    pub ttl_hint: String,
    #[serde(default)]
    pub should_publish: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshDistilledClaim {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub stance: String,
    #[serde(default)]
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshDistilledEntity {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub entity_type: String,
}

#[derive(Debug, Clone)]
pub struct FreshDistillResult {
    pub distilled: FreshDistilledItem,
    pub llm_input_tokens: Option<u32>,
    pub llm_output_tokens: Option<u32>,
}

#[async_trait]
pub trait FreshContextDistiller: Send + Sync {
    async fn distill(&self, input: &FreshDistillInput) -> Result<FreshDistillResult, AppError>;
}

fn default_content_type() -> String {
    "other".into()
}

fn default_rumor_level() -> String {
    rumor_level::RUMOR.into()
}

fn default_ttl_hint() -> String {
    source_kind::NEWS.into()
}

#[cfg(test)]
mod tests {
    use crate::shared::llm_json::parse_llm_json;

    use super::*;

    #[test]
    fn missing_fresh_classification_fields_use_conservative_defaults() {
        let parsed: FreshDistilledItem = parse_llm_json(
            r#"{
              "accept": false,
              "title": "",
              "summary": "",
              "claims": [],
              "entities": [],
              "should_publish": false
            }"#,
        )
        .unwrap();

        assert_eq!(parsed.content_type, "other");
        assert_eq!(parsed.rumor_level, rumor_level::RUMOR);
        assert_eq!(parsed.ttl_hint, source_kind::NEWS);
        assert!(!parsed.should_publish);
    }
}
