//! Domain event type constants for the outbox-driven web ingestion pipeline.
//!
//! Every event type is a fixed string used as `event_type` in `domain_event_outbox`.

pub mod event {
    // ── Main pipeline ────────────────────────────────────────────────────
    pub const CRAWL_JOB_CREATED: &str = "CrawlJobCreated";
    pub const URL_DISCOVERED: &str = "UrlDiscovered";
    pub const PAGE_FETCHED: &str = "PageFetched";
    pub const PAGE_CLEANED: &str = "PageCleaned";
    pub const PAGE_DISTILLED: &str = "PageDistilled";
    pub const QUALITY_CHECKED: &str = "QualityChecked";
    pub const DOCUMENT_CHUNKED: &str = "DocumentChunked";
    pub const CHUNKS_EMBEDDED: &str = "ChunksEmbedded";
    pub const DOCUMENT_INDEXED: &str = "DocumentIndexed";
    pub const KNOWLEDGE_STAGED: &str = "KnowledgeStaged";
    pub const KNOWLEDGE_PUBLISH_REQUESTED: &str = "KnowledgePublishRequested";
    pub const KNOWLEDGE_PUBLISHED: &str = "KnowledgePublished";
    pub const KNOWLEDGE_SUPERSEDED: &str = "KnowledgeSuperseded";

    // ── Rollback ─────────────────────────────────────────────────────────
    pub const KNOWLEDGE_ROLLBACK_REQUESTED: &str = "KnowledgeRollbackRequested";
    pub const KNOWLEDGE_ROLLED_BACK: &str = "KnowledgeRolledBack";

    // ── Terminal / skip ──────────────────────────────────────────────────
    pub const INGESTION_SKIPPED: &str = "IngestionSkipped";
    pub const INGESTION_REJECTED: &str = "IngestionRejected";
    pub const INGESTION_FAILED: &str = "IngestionFailed";
    pub const INGESTION_DEAD: &str = "IngestionDead";
}

/// Aggregate type strings used in `domain_event_outbox.aggregate_type`.
pub mod aggregate {
    pub const WEB_CRAWL_JOB: &str = "web_crawl_job";
    pub const WEB_PAGE: &str = "web_page";
    pub const KNOWLEDGE_INGESTION_RUN: &str = "knowledge_ingestion_run";
    pub const KNOWLEDGE_PUBLISH_RECORD: &str = "knowledge_publish_record";
}
