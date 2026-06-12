//! Event handlers — one module per event type (task-book §4.2).
//!
//! Each handler takes `&DomainEvent` + `&PipelineContext` and is responsible for
//! exactly one event type. Handlers are idempotent and resumable (§5.8).

pub mod crawl_job_created;
pub mod page_cleaned;
pub mod page_distilled;
pub mod page_fetched;
pub mod quality_checked;
pub mod terminal;
pub mod unimplemented;
pub mod url_discovered;

// Back-half handlers (Phase 2/3) — implemented in later phases.
pub mod chunks_embedded;
pub mod document_chunked;
pub mod document_indexed;
pub mod knowledge_staged;
pub mod publish_requested;
pub mod rollback_requested;
