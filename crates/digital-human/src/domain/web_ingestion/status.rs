//! 网页知识摄取实体的状态和阶段常量。
//!
//! All status/stage values are defined here as string constants so the
//! application layer does not scatter magic strings.

// ── web_sources.approval_status ──────────────────────────────────────────────

pub mod source_approval {
    pub const PENDING: &str = "pending";
    pub const APPROVED: &str = "approved";
    pub const REJECTED: &str = "rejected";
    pub const DISABLED: &str = "disabled";
}

// ── web_sources.trust_level ──────────────────────────────────────────────────

pub mod source_trust {
    pub const OFFICIAL: &str = "official";
    pub const TRUSTED: &str = "trusted";
    pub const NORMAL: &str = "normal";
    pub const UNTRUSTED: &str = "untrusted";
}

// ── web_crawl_jobs.status ────────────────────────────────────────────────────

pub mod crawl_job_status {
    pub const PENDING: &str = "pending";
    pub const RUNNING: &str = "running";
    pub const SUCCEEDED: &str = "succeeded";
    pub const FAILED: &str = "failed";
    pub const DEAD: &str = "dead";
    pub const CANCELLED: &str = "cancelled";
}

// ── knowledge_ingestion_runs.status ──────────────────────────────────────────

pub mod run_status {
    pub const PENDING: &str = "pending";
    pub const RUNNING: &str = "running";
    pub const STAGED: &str = "staged";
    pub const PUBLISHED: &str = "published";
    pub const REJECTED: &str = "rejected";
    pub const SKIPPED: &str = "skipped";
    pub const FAILED: &str = "failed";
    pub const DEAD: &str = "dead";
    pub const CANCELLED: &str = "cancelled";
}

// ── knowledge_ingestion_runs.stage ───────────────────────────────────────────

pub mod run_stage {
    pub const PENDING: &str = "pending";
    pub const FETCHING: &str = "fetching";
    pub const FETCHED: &str = "fetched";
    pub const UNCHANGED: &str = "unchanged";
    pub const CLEANING: &str = "cleaning";
    pub const CLEANED: &str = "cleaned";
    pub const DISTILLING: &str = "distilling";
    pub const DISTILLED: &str = "distilled";
    pub const QUALITY_CHECKED: &str = "quality_checked";
    pub const CHUNKING: &str = "chunking";
    pub const CHUNKED: &str = "chunked";
    pub const EMBEDDING: &str = "embedding";
    pub const EMBEDDED: &str = "embedded";
    pub const INDEXING: &str = "indexing";
    pub const INDEXED: &str = "indexed";
    pub const STAGING: &str = "staging";
    pub const PUBLISHING: &str = "publishing";
    pub const PUBLISHED: &str = "published";
    pub const REJECTED: &str = "rejected";
    pub const SKIPPED: &str = "skipped";
    pub const FAILED: &str = "failed";
    pub const DEAD: &str = "dead";
    pub const CANCELLED: &str = "cancelled";
}

// ── knowledge_publish_records.publish_status ─────────────────────────────────

pub mod publish_status {
    pub const STAGED: &str = "staged";
    pub const PUBLISHING: &str = "publishing";
    pub const PUBLISHED: &str = "published";
    pub const SUPERSEDED: &str = "superseded";
    pub const ROLLED_BACK: &str = "rolled_back";
    pub const FAILED: &str = "failed";
}

// ── domain_event_outbox.status ───────────────────────────────────────────────

pub mod outbox_status {
    pub const PENDING: &str = "pending";
    pub const PROCESSING: &str = "processing";
    pub const PUBLISHED: &str = "published";
    pub const FAILED: &str = "failed";
    pub const DEAD: &str = "dead";
}

// ── knowledge_chunk_manifests.chunk_type ─────────────────────────────────────

pub mod chunk_type {
    pub const DOCUMENT_SUMMARY: &str = "document_summary";
    pub const SECTION_SUMMARY: &str = "section_summary";
    pub const ATOMIC: &str = "atomic";
}

// ── audit_log.action ─────────────────────────────────────────────────────────

pub mod audit_action {
    pub const CONTENT_UNCHANGED: &str = "content_unchanged";
    pub const QUALITY_REJECTED: &str = "quality_rejected";
    pub const MANUAL_REVIEW_REQUIRED: &str = "manual_review_required";
    pub const PUBLISH_STARTED: &str = "publish_started";
    pub const PUBLISH_SUCCEEDED: &str = "publish_succeeded";
    pub const PUBLISH_FAILED: &str = "publish_failed";
    pub const ROLLBACK_STARTED: &str = "rollback_started";
    pub const ROLLBACK_SUCCEEDED: &str = "rollback_succeeded";
    pub const ROLLBACK_FAILED: &str = "rollback_failed";
    pub const VECTOR_SYNC_FAILED: &str = "vector_sync_failed";
}

// ── Known terminal (final) statuses ──────────────────────────────────────────

/// 终态的摄取运行状态 (no further transitions allowed).
pub fn is_terminal_run_status(s: &str) -> bool {
    matches!(
        s,
        run_status::PUBLISHED
            | run_status::REJECTED
            | run_status::SKIPPED
            | run_status::FAILED
            | run_status::DEAD
            | run_status::CANCELLED
    )
}

/// 终态的发布记录状态。
pub fn is_terminal_publish_status(s: &str) -> bool {
    matches!(
        s,
        publish_status::PUBLISHED
            | publish_status::SUPERSEDED
            | publish_status::ROLLED_BACK
            | publish_status::FAILED
    )
}
