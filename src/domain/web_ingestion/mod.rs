//! Web Ingestion domain module — status constants, event types, state machine,
//! value objects, error types, and repository traits.
//!
//! Design principle: DB state machine is the authoritative source of truth.
//! Outbox events drive the pipeline. Each worker is idempotent.

pub mod distiller;
pub mod error;
pub mod event_types;
pub mod fetcher;
pub mod repository;
pub mod review;
pub mod state_machine;
pub mod status;

pub use distiller::*;
pub use error::*;
pub use event_types::*;
pub use fetcher::*;
pub use repository::*;
pub use review::*;
pub use state_machine::*;
pub use status::*;
