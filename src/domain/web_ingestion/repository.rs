//! 网页知识摄取领域的仓库接口。
//!
//! Each trait corresponds to one aggregate / table. They live in the domain
//! layer and are implemented by SeaORM repositories in the infrastructure layer.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;

use super::error::WebIngestionError;

// ── web_sources ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WebSource {
    pub id: u64,
    pub name: String,
    pub description: Option<String>,
    pub approval_status: String,
    pub trust_level: String,
    pub auto_publish: bool,
    pub allowed_domains: Option<JsonValue>,
    pub default_language: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewWebSource {
    pub name: String,
    pub description: Option<String>,
    pub approval_status: String,
    pub trust_level: String,
    pub auto_publish: bool,
    pub allowed_domains: Option<JsonValue>,
    pub default_language: String,
    pub enabled: bool,
}

#[async_trait]
pub trait WebSourceRepoT: Send + Sync {
    async fn find_by_id(&self, id: u64) -> Result<Option<WebSource>, WebIngestionError>;
    async fn list_enabled(&self) -> Result<Vec<WebSource>, WebIngestionError>;
    async fn insert(&self, source: NewWebSource) -> Result<WebSource, WebIngestionError>;
    async fn update(&self, id: u64, source: NewWebSource) -> Result<WebSource, WebIngestionError>;
}

// ── web_source_urls ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WebSourceUrl {
    pub id: u64,
    pub source_id: u64,
    pub url: String,
    pub canonical_url: Option<String>,
    pub url_hash: String,
    pub enabled: bool,
    pub crawl_interval_secs: u32,
    pub last_crawled_at: Option<DateTime<Utc>>,
    pub last_content_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewWebSourceUrl {
    pub source_id: u64,
    pub url: String,
    pub canonical_url: Option<String>,
    pub url_hash: String,
    pub crawl_interval_secs: u32,
}

#[async_trait]
pub trait WebSourceUrlRepoT: Send + Sync {
    async fn find_by_id(&self, id: u64) -> Result<Option<WebSourceUrl>, WebIngestionError>;
    async fn find_by_source_and_hash(
        &self,
        source_id: u64,
        url_hash: &str,
    ) -> Result<Option<WebSourceUrl>, WebIngestionError>;
    async fn list_by_source(&self, source_id: u64) -> Result<Vec<WebSourceUrl>, WebIngestionError>;
    async fn list_due_for_crawl(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<WebSourceUrl>, WebIngestionError>;
    async fn upsert(&self, url: NewWebSourceUrl) -> Result<WebSourceUrl, WebIngestionError>;
    async fn mark_crawled(
        &self,
        id: u64,
        content_hash: &str,
        crawled_at: DateTime<Utc>,
    ) -> Result<(), WebIngestionError>;
}

// ── web_crawl_jobs ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WebCrawlJob {
    pub id: u64,
    pub source_id: Option<u64>,
    pub status: String,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewWebCrawlJob {
    pub source_id: Option<u64>,
    pub status: String,
    pub scheduled_at: DateTime<Utc>,
}

#[async_trait]
pub trait WebCrawlJobRepoT: Send + Sync {
    async fn find_by_id(&self, id: u64) -> Result<Option<WebCrawlJob>, WebIngestionError>;
    async fn insert(&self, job: NewWebCrawlJob) -> Result<WebCrawlJob, WebIngestionError>;
    async fn update_status(
        &self,
        id: u64,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<(), WebIngestionError>;
    async fn mark_started(&self, id: u64) -> Result<(), WebIngestionError>;
    async fn mark_finished(&self, id: u64, status: &str) -> Result<(), WebIngestionError>;
}

// ── web_pages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WebPage {
    pub id: u64,
    pub source_id: u64,
    pub source_url_id: Option<u64>,
    pub url: String,
    pub canonical_url: Option<String>,
    pub url_hash: String,
    pub latest_content_hash: Option<String>,
    pub latest_success_run_id: Option<u64>,
    pub last_fetched_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewWebPage {
    pub source_id: u64,
    pub source_url_id: Option<u64>,
    pub url: String,
    pub canonical_url: Option<String>,
    pub url_hash: String,
}

#[async_trait]
pub trait WebPageRepoT: Send + Sync {
    async fn find_by_id(&self, id: u64) -> Result<Option<WebPage>, WebIngestionError>;
    async fn find_by_source_and_hash(
        &self,
        source_id: u64,
        url_hash: &str,
    ) -> Result<Option<WebPage>, WebIngestionError>;
    async fn upsert(&self, page: NewWebPage) -> Result<WebPage, WebIngestionError>;
    async fn mark_fetched(
        &self,
        id: u64,
        content_hash: &str,
        run_id: u64,
        fetched_at: DateTime<Utc>,
    ) -> Result<(), WebIngestionError>;
}

// ── knowledge_ingestion_runs ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KnowledgeIngestionRun {
    pub id: u64,
    pub source_id: u64,
    pub source_url_id: Option<u64>,
    pub crawl_job_id: Option<u64>,
    pub page_id: u64,
    pub content_hash: String,
    pub content_key: String,
    pub run_key: String,
    pub version_key: String,
    pub status: String,
    pub stage: String,
    pub llm_provider: Option<String>,
    pub llm_model: Option<String>,
    pub llm_prompt_version: Option<String>,
    pub llm_input_tokens: Option<u32>,
    pub llm_output_tokens: Option<u32>,
    pub chunker_version: Option<String>,
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    pub embedding_dimension: Option<u32>,
    pub quality_score: Option<f64>,
    pub quality_result: Option<JsonValue>,
    pub risk_flags: Option<JsonValue>,
    pub should_publish: Option<bool>,
    pub last_error: Option<String>,
    pub retry_count: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    // ── Mid-pipeline artifacts ──
    pub fetched_body_text: Option<String>,
    pub clean_text: Option<String>,
    pub distilled_json: Option<JsonValue>,
}

#[derive(Debug, Clone)]
pub struct NewIngestionRun {
    pub source_id: u64,
    pub source_url_id: Option<u64>,
    pub crawl_job_id: Option<u64>,
    pub page_id: u64,
    pub content_hash: String,
    pub content_key: String,
    pub run_key: String,
    pub version_key: String,
}

#[async_trait]
pub trait IngestionRunRepoT: Send + Sync {
    async fn find_by_id(&self, id: u64)
    -> Result<Option<KnowledgeIngestionRun>, WebIngestionError>;
    async fn find_by_run_key(
        &self,
        run_key: &str,
    ) -> Result<Option<KnowledgeIngestionRun>, WebIngestionError>;
    async fn find_by_content_key(
        &self,
        content_key: &str,
    ) -> Result<Option<KnowledgeIngestionRun>, WebIngestionError>;
    async fn insert(
        &self,
        run: NewIngestionRun,
    ) -> Result<KnowledgeIngestionRun, WebIngestionError>;
    async fn update_status_stage(
        &self,
        id: u64,
        expected_status: &str,
        expected_stage: &str,
        new_status: &str,
        new_stage: &str,
        last_error: Option<&str>,
    ) -> Result<bool, WebIngestionError>;
    async fn update_distill_result(
        &self,
        id: u64,
        llm_provider: &str,
        llm_model: &str,
        llm_prompt_version: &str,
        llm_input_tokens: Option<u32>,
        llm_output_tokens: Option<u32>,
        quality_score: f64,
        quality_result: JsonValue,
        risk_flags: JsonValue,
        should_publish: bool,
    ) -> Result<(), WebIngestionError>;
    async fn update_embedding_info(
        &self,
        id: u64,
        embedding_provider: &str,
        embedding_model: &str,
        embedding_dimension: u32,
    ) -> Result<(), WebIngestionError>;
    async fn mark_started(&self, id: u64) -> Result<(), WebIngestionError>;
    async fn mark_finished(&self, id: u64) -> Result<(), WebIngestionError>;
    /// Update mid-pipeline artifacts (fetched body, clean text, distilled JSON).
    /// Only sets the fields that are Some; None means "don't update this field".
    async fn update_artifacts(
        &self,
        id: u64,
        fetched_body_text: Option<&str>,
        clean_text: Option<&str>,
        distilled_json: Option<JsonValue>,
    ) -> Result<(), WebIngestionError>;
    async fn find_latest_for_page(
        &self,
        page_id: u64,
    ) -> Result<Option<KnowledgeIngestionRun>, WebIngestionError>;
}

// ── knowledge_publish_records ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KnowledgePublishRecord {
    pub id: u64,
    pub source_id: u64,
    pub page_id: u64,
    pub run_id: u64,
    pub document_id: u64,
    pub version_key: String,
    pub content_hash: String,
    pub publish_status: String,
    pub active: bool,
    pub active_page_key: Option<String>,
    pub activated_at: Option<DateTime<Utc>>,
    pub superseded_at: Option<DateTime<Utc>>,
    pub superseded_by_record_id: Option<u64>,
    pub rolled_back_from_record_id: Option<u64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewPublishRecord {
    pub source_id: u64,
    pub page_id: u64,
    pub run_id: u64,
    pub document_id: u64,
    pub version_key: String,
    pub content_hash: String,
    pub active_page_key: Option<String>,
}

#[async_trait]
pub trait PublishRecordRepoT: Send + Sync {
    async fn find_by_id(
        &self,
        id: u64,
    ) -> Result<Option<KnowledgePublishRecord>, WebIngestionError>;
    async fn find_active_by_page(
        &self,
        source_id: u64,
        page_id: u64,
    ) -> Result<Option<KnowledgePublishRecord>, WebIngestionError>;
    async fn find_by_run_id(
        &self,
        run_id: u64,
    ) -> Result<Option<KnowledgePublishRecord>, WebIngestionError>;
    async fn insert(
        &self,
        record: NewPublishRecord,
    ) -> Result<KnowledgePublishRecord, WebIngestionError>;
    async fn set_active(
        &self,
        id: u64,
        active: bool,
        active_page_key: Option<&str>,
        publish_status: &str,
    ) -> Result<(), WebIngestionError>;
    /// Find the record that is currently active for the same page as `record_id`.
    async fn find_active_sibling(
        &self,
        record_id: u64,
    ) -> Result<Option<KnowledgePublishRecord>, WebIngestionError>;
    /// 为页面发布/回滚获取事务锁。
    async fn lock_page_for_publish(
        &self,
        source_id: u64,
        page_id: u64,
    ) -> Result<(), WebIngestionError>;

    /// Atomically publish a staged record (task-book §12.1, §12.6-8).
    ///
    /// In ONE DB transaction with a `web_pages` FOR UPDATE lock:
    ///   1. verify the target record is staged
    ///   2. supersede the current active record (active=0, status=superseded,
    ///      its knowledge_documents.status→0, its manifests active=0)
    ///   3. activate the target (active=1, status=published, activated_at,
    ///      its knowledge_documents.status→1, its manifests active=1)
    ///
    /// Returns the publish outcome (which record was superseded, if any).
    /// DB state is authoritative; the caller re-syncs Qdrant afterwards.
    async fn publish_in_tx(
        &self,
        publish_record_id: u64,
    ) -> Result<PublishOutcome, WebIngestionError>;

    /// Atomically roll back to a previous version (task-book §12.3).
    ///
    /// In ONE DB transaction with a page lock: deactivate `current_record_id`
    /// (status=rolled_back, doc.status→0, manifests active=0) and reactivate
    /// `target_record_id` (status=published, doc.status→1, manifests active=1).
    async fn rollback_in_tx(
        &self,
        current_record_id: u64,
        target_record_id: u64,
    ) -> Result<PublishOutcome, WebIngestionError>;
}

/// Outcome of a transactional publish / rollback.
#[derive(Debug, Clone)]
pub struct PublishOutcome {
    /// The record that became active.
    pub activated_record_id: u64,
    pub activated_document_id: u64,
    /// The record that was deactivated/superseded (None on a first publish).
    pub deactivated_record_id: Option<u64>,
    pub deactivated_document_id: Option<u64>,
    /// True when the target was already active (idempotent no-op publish).
    pub was_already_active: bool,
}

// ── knowledge_chunk_manifests ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KnowledgeChunkManifest {
    pub id: u64,
    pub publish_record_id: u64,
    pub run_id: u64,
    pub document_id: u64,
    pub chunk_id: u64,
    pub version_key: String,
    pub chunk_hash: String,
    pub chunk_type: String,
    pub chunk_index: u32,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewChunkManifest {
    pub publish_record_id: u64,
    pub run_id: u64,
    pub document_id: u64,
    pub chunk_id: u64,
    pub version_key: String,
    pub chunk_hash: String,
    pub chunk_type: String,
    pub chunk_index: u32,
}

#[async_trait]
pub trait ChunkManifestRepoT: Send + Sync {
    async fn find_by_version_and_hash(
        &self,
        version_key: &str,
        chunk_hash: &str,
    ) -> Result<Option<KnowledgeChunkManifest>, WebIngestionError>;
    async fn find_by_chunk_id(
        &self,
        chunk_id: u64,
    ) -> Result<Option<KnowledgeChunkManifest>, WebIngestionError>;
    async fn insert_batch(
        &self,
        manifests: &[NewChunkManifest],
    ) -> Result<Vec<KnowledgeChunkManifest>, WebIngestionError>;
    async fn set_active_by_publish_record(
        &self,
        publish_record_id: u64,
        active: bool,
    ) -> Result<(), WebIngestionError>;
    async fn list_by_publish_record(
        &self,
        publish_record_id: u64,
    ) -> Result<Vec<KnowledgeChunkManifest>, WebIngestionError>;
}

// ── knowledge_vector_manifests ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KnowledgeVectorManifest {
    pub id: u64,
    pub publish_record_id: u64,
    pub run_id: u64,
    pub document_id: u64,
    pub chunk_id: u64,
    pub chunk_hash: String,
    pub qdrant_collection: String,
    pub qdrant_point_id: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_dimension: u32,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewVectorManifest {
    pub publish_record_id: u64,
    pub run_id: u64,
    pub document_id: u64,
    pub chunk_id: u64,
    pub chunk_hash: String,
    pub qdrant_collection: String,
    pub qdrant_point_id: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_dimension: u32,
}

#[async_trait]
pub trait VectorManifestRepoT: Send + Sync {
    async fn find_by_collection_and_point(
        &self,
        collection: &str,
        point_id: &str,
    ) -> Result<Option<KnowledgeVectorManifest>, WebIngestionError>;
    async fn find_by_chunk_and_model(
        &self,
        chunk_id: u64,
        embedding_model: &str,
    ) -> Result<Option<KnowledgeVectorManifest>, WebIngestionError>;
    async fn insert_batch(
        &self,
        manifests: &[NewVectorManifest],
    ) -> Result<Vec<KnowledgeVectorManifest>, WebIngestionError>;
    async fn set_active_by_publish_record(
        &self,
        publish_record_id: u64,
        active: bool,
    ) -> Result<(), WebIngestionError>;
    async fn list_by_publish_record(
        &self,
        publish_record_id: u64,
    ) -> Result<Vec<KnowledgeVectorManifest>, WebIngestionError>;
}

// ── domain_event_outbox ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DomainEvent {
    pub id: u64,
    pub event_key: String,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: u64,
    pub payload: JsonValue,
    pub status: String,
    pub retry_count: u32,
    pub max_retries: u32,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub locked_by: Option<String>,
    pub locked_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewOutboxEvent {
    pub event_key: String,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: u64,
    pub payload: JsonValue,
    pub max_retries: u32,
}

#[async_trait]
pub trait OutboxRepoT: Send + Sync {
    /// Insert an event, relying on UNIQUE(event_key) for idempotency.
    async fn insert_event(&self, event: NewOutboxEvent) -> Result<DomainEvent, WebIngestionError>;
    /// Atomically claim a batch of pending/failed/timed-out events.
    async fn claim_batch(
        &self,
        claim_token: &str,
        lock_ttl_secs: u32,
        limit: u64,
    ) -> Result<Vec<DomainEvent>, WebIngestionError>;
    /// Mark a claimed event as published (success).
    async fn mark_published(&self, id: u64, claim_token: &str) -> Result<bool, WebIngestionError>;
    /// Mark a claimed event as failed (retryable) or dead (retries exhausted).
    async fn mark_failed_or_dead(
        &self,
        id: u64,
        claim_token: &str,
        last_error: &str,
        next_retry_at: DateTime<Utc>,
        is_dead: bool,
    ) -> Result<bool, WebIngestionError>;
    /// List all events for a given aggregate.
    async fn list_by_aggregate(
        &self,
        aggregate_type: &str,
        aggregate_id: u64,
    ) -> Result<Vec<DomainEvent>, WebIngestionError>;
}

// ── web_ingestion_audit_logs ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AuditLog {
    pub id: u64,
    pub source_id: Option<u64>,
    pub source_url_id: Option<u64>,
    pub page_id: Option<u64>,
    pub run_id: Option<u64>,
    pub publish_record_id: Option<u64>,
    pub action: String,
    pub status: String,
    pub message: String,
    pub metadata: Option<JsonValue>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewAuditLog {
    pub source_id: Option<u64>,
    pub source_url_id: Option<u64>,
    pub page_id: Option<u64>,
    pub run_id: Option<u64>,
    pub publish_record_id: Option<u64>,
    pub action: String,
    pub status: String,
    pub message: String,
    pub metadata: Option<JsonValue>,
}

#[async_trait]
pub trait AuditLogRepoT: Send + Sync {
    async fn insert(&self, log: NewAuditLog) -> Result<AuditLog, WebIngestionError>;
    async fn list_by_run(&self, run_id: u64) -> Result<Vec<AuditLog>, WebIngestionError>;
    async fn list_by_publish_record(
        &self,
        publish_record_id: u64,
    ) -> Result<Vec<AuditLog>, WebIngestionError>;
}
