use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::shared::error::AppError;

pub mod distiller;

pub use distiller::*;

pub mod source_kind {
    pub const NEWS: &str = "news";
    pub const RSS: &str = "rss";
    pub const TREND: &str = "trend";
    pub const GOSSIP: &str = "gossip";
    pub const FORUM: &str = "forum";
    pub const SOCIAL: &str = "social";
    pub const SEARCH: &str = "search";
}

pub mod rumor_level {
    pub const CONFIRMED: &str = "confirmed";
    pub const REPORTED: &str = "reported";
    pub const RUMOR: &str = "rumor";
    pub const DISPUTED: &str = "disputed";
}

pub mod fresh_status {
    pub const FETCHED: &str = "fetched";
    pub const DISTILLED: &str = "distilled";
    pub const PUBLISHED: &str = "published";
    pub const EXPIRED: &str = "expired";
    pub const REJECTED: &str = "rejected";
}

pub mod risk_policy {
    pub const NORMAL: &str = "normal";
    pub const STRICT: &str = "strict";
    pub const MANUAL_REVIEW: &str = "manual_review";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshSource {
    pub id: u64,
    pub name: String,
    pub source_kind: String,
    pub base_url: Option<String>,
    pub allowed_domains: Option<serde_json::Value>,
    pub trust_level: String,
    pub reliability_score: f64,
    pub crawl_interval_secs: u32,
    pub default_ttl_secs: u32,
    pub risk_policy: String,
    pub enabled: i8,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewFreshSource {
    pub name: String,
    pub source_kind: String,
    pub base_url: Option<String>,
    pub allowed_domains: Option<serde_json::Value>,
    pub trust_level: String,
    pub reliability_score: f64,
    pub crawl_interval_secs: u32,
    pub default_ttl_secs: u32,
    pub risk_policy: String,
    pub enabled: i8,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshItem {
    pub id: u64,
    pub source_id: u64,
    pub url: Option<String>,
    pub canonical_url: Option<String>,
    pub url_hash: Option<String>,
    pub title: Option<String>,
    pub raw_text: Option<String>,
    pub clean_text: Option<String>,
    pub summary: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub content_hash: String,
    pub status: String,
    pub reliability_score: f64,
    pub freshness_score: f64,
    pub heat_score: f64,
    pub rumor_level: String,
    pub risk_flags: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewFreshItem {
    pub source_id: u64,
    pub url: Option<String>,
    pub canonical_url: Option<String>,
    pub url_hash: Option<String>,
    pub title: Option<String>,
    pub raw_text: Option<String>,
    pub clean_text: Option<String>,
    pub summary: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub content_hash: String,
    pub status: String,
    pub reliability_score: f64,
    pub freshness_score: f64,
    pub heat_score: f64,
    pub rumor_level: String,
    pub risk_flags: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshTopic {
    pub id: u64,
    pub topic_key: String,
    pub title: String,
    pub summary: Option<String>,
    pub entities: Option<serde_json::Value>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub heat_score: f64,
    pub freshness_score: f64,
    pub expires_at: DateTime<Utc>,
    pub status: String,
    pub risk_flags: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewFreshTopic {
    pub topic_key: String,
    pub title: String,
    pub summary: Option<String>,
    pub entities: Option<serde_json::Value>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub heat_score: f64,
    pub freshness_score: f64,
    pub expires_at: DateTime<Utc>,
    pub status: String,
    pub risk_flags: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshChunk {
    pub id: u64,
    pub item_id: u64,
    pub topic_id: Option<u64>,
    pub chunk_index: u32,
    pub content: String,
    pub content_hash: String,
    pub token_count: Option<u32>,
    pub metadata: Option<serde_json::Value>,
    pub vector_id: Option<String>,
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    pub embedding_dimension: Option<u32>,
    pub active: i8,
    pub indexed_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewFreshChunk {
    pub item_id: u64,
    pub topic_id: Option<u64>,
    pub chunk_index: u32,
    pub content: String,
    pub content_hash: String,
    pub token_count: Option<u32>,
    pub metadata: Option<serde_json::Value>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshTopicEvidence {
    pub topic_id: u64,
    pub item_id: u64,
    pub stance: String,
    pub confidence: f64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewFreshTopicEvidence {
    pub topic_id: u64,
    pub item_id: u64,
    pub stance: String,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub struct FreshItemDistillUpdate {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub freshness_score: f64,
    pub heat_score: f64,
    pub rumor_level: String,
    pub risk_flags: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct FreshFetchResult {
    pub final_url: String,
    pub content_type: Option<String>,
    pub body_text: String,
    pub content_length: Option<u64>,
}

#[async_trait]
pub trait FreshContentFetcher: Send + Sync {
    async fn fetch(
        &self,
        url: &str,
        allowed_domains: Option<&[String]>,
    ) -> Result<FreshFetchResult, AppError>;
}

#[async_trait]
pub trait FreshContextRepoT: Send + Sync {
    async fn insert_source(&self, source: NewFreshSource) -> Result<FreshSource, AppError>;
    async fn list_enabled_sources(&self, limit: u64) -> Result<Vec<FreshSource>, AppError>;
    async fn find_source_by_id(&self, source_id: u64) -> Result<Option<FreshSource>, AppError>;

    async fn insert_item(&self, item: NewFreshItem) -> Result<FreshItem, AppError>;
    async fn find_item_by_source_content(
        &self,
        source_id: u64,
        content_hash: &str,
    ) -> Result<Option<FreshItem>, AppError>;
    async fn find_item_by_id(&self, item_id: u64) -> Result<Option<FreshItem>, AppError>;
    async fn list_active_items(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<FreshItem>, AppError>;
    async fn list_chunkable_items(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<FreshItem>, AppError>;
    async fn list_items_by_status(
        &self,
        status: &str,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<FreshItem>, AppError>;
    async fn expire_items(&self, now: DateTime<Utc>) -> Result<u64, AppError>;
    async fn update_item_status_if_current(
        &self,
        item_id: u64,
        expected_status: &str,
        new_status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<bool, AppError>;
    async fn update_item_distill_result_if_current(
        &self,
        item_id: u64,
        expected_status: &str,
        new_status: &str,
        update: FreshItemDistillUpdate,
    ) -> Result<bool, AppError>;

    async fn insert_topic(&self, topic: NewFreshTopic) -> Result<FreshTopic, AppError>;
    async fn upsert_topic(&self, topic: NewFreshTopic) -> Result<FreshTopic, AppError>;
    async fn find_topic_by_key(&self, topic_key: &str) -> Result<Option<FreshTopic>, AppError>;
    async fn link_topic_evidence(
        &self,
        evidence: NewFreshTopicEvidence,
    ) -> Result<FreshTopicEvidence, AppError>;
    async fn assign_topic_to_item_chunks(
        &self,
        item_id: u64,
        topic_id: u64,
    ) -> Result<u64, AppError>;

    async fn insert_chunks(&self, chunks: &[NewFreshChunk]) -> Result<Vec<FreshChunk>, AppError>;
    async fn find_chunk_by_id(&self, chunk_id: u64) -> Result<Option<FreshChunk>, AppError>;
    async fn find_chunks_by_item(&self, item_id: u64) -> Result<Vec<FreshChunk>, AppError>;
    async fn list_indexable_chunks(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<FreshChunk>, AppError>;
    async fn mark_chunk_indexed(
        &self,
        chunk_id: u64,
        vector_id: String,
        embedding_provider: String,
        embedding_model: String,
        embedding_dimension: u32,
    ) -> Result<bool, AppError>;
    async fn list_expired_indexed_chunks(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<FreshChunk>, AppError>;
    async fn mark_chunk_vector_deleted(
        &self,
        chunk_id: u64,
        vector_id: &str,
    ) -> Result<bool, AppError>;
}
