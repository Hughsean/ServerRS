//! Web ingestion application layer — services that implement the pipeline.
//!
//! Architecture (task-book required):
//!   Scheduler → OutboxDispatcher → Handlers (each idempotent + resumable)
//!   Handlers read/write the DB state machine as the authoritative source of
//!   truth. The outbox carries only ids + small metadata, never large text.
//!
//! Module layout (§4):
//!   - `dispatcher`            — claim / route / mark published|failed|dead
//!   - `scheduler`             — periodic crawl-job creation
//!   - `pipeline_context`      — shared dependency bundle for handlers
//!   - `event_types`           — re-export of domain event/aggregate constants
//!   - `state_machine_adapter` — centralized guarded transitions
//!   - `handlers/`             — one module per event type
//!   - `services/`             — stateless helpers (run_key, profile, etc.)

pub mod dispatcher;
pub mod event_types;
pub mod handlers;
pub mod pipeline_context;
pub mod scheduler;
pub mod services;
pub mod state_machine_adapter;

// Stateless domain services used across handlers.
pub mod distill_service;
pub mod extractor;
pub mod hash;
pub mod industrial_chunker;
pub mod quality_gate;

// Re-exports
pub use hash::*;
pub use pipeline_context::PipelineContext;
