//! Web ingestion application layer — services that implement the pipeline.
//!
//! Architecture (task-book required):
//!   Scheduler → OutboxDispatcher → Workers (each idempotent)
//!   Workers read/write DB state machine as authoritative source of truth.

pub mod distill_service;
pub mod extractor;
pub mod hash;
pub mod industrial_chunker;
pub mod publish_service;
pub mod quality_gate;

// Re-exports
pub use hash::*;
