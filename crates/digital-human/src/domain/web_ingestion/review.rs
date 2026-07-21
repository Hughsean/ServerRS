use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;

use super::error::WebIngestionError;

#[derive(Debug, Clone)]
pub struct KnowledgeReviewFilter {
    pub publish_status: String,
    pub source_id: Option<u64>,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Clone)]
pub struct KnowledgeReviewItem {
    pub publish_record_id: u64,
    pub source_id: u64,
    pub source_name: String,
    pub page_id: u64,
    pub run_id: u64,
    pub document_id: u64,
    pub version_key: String,
    pub title: Option<String>,
    pub source_url: String,
    pub publish_status: String,
    pub active: bool,
    pub run_status: String,
    pub run_stage: String,
    pub quality_score: Option<f64>,
    pub quality_result: Option<JsonValue>,
    pub risk_flags: Option<JsonValue>,
    pub should_publish: Option<bool>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct KnowledgeReviewPage {
    pub items: Vec<KnowledgeReviewItem>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
}

#[derive(Debug, Clone)]
pub struct KnowledgeReviewAuditEntry {
    pub action: String,
    pub status: String,
    pub message: String,
    pub metadata: Option<JsonValue>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct KnowledgeReviewDetail {
    pub review: KnowledgeReviewItem,
    pub clean_text: Option<String>,
    pub distilled_json: Option<JsonValue>,
    pub audit_logs: Vec<KnowledgeReviewAuditEntry>,
}

#[derive(Debug, Clone)]
pub struct NewReviewPublishRequest {
    pub publish_record_id: u64,
    pub event_key: String,
    pub reviewer_user_id: u64,
    pub reviewer_username: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReviewPublishRequest {
    pub publish_record_id: u64,
    pub event_id: u64,
    pub event_status: String,
    pub already_requested: bool,
}

#[async_trait]
pub trait KnowledgeReviewRepoT: Send + Sync {
    async fn list(
        &self,
        filter: KnowledgeReviewFilter,
    ) -> Result<KnowledgeReviewPage, WebIngestionError>;

    async fn find_item_by_id(
        &self,
        publish_record_id: u64,
    ) -> Result<Option<KnowledgeReviewItem>, WebIngestionError>;

    async fn find_detail_by_id(
        &self,
        publish_record_id: u64,
    ) -> Result<Option<KnowledgeReviewDetail>, WebIngestionError>;

    async fn request_publish(
        &self,
        request: NewReviewPublishRequest,
    ) -> Result<ReviewPublishRequest, WebIngestionError>;

    // ── Statistics ──
    async fn count_all(&self) -> Result<u64, WebIngestionError>;
    async fn count_trend(&self, days: u32) -> Result<Vec<(String, u64)>, WebIngestionError>;
}
