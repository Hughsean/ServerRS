//! Re-export of domain event/aggregate constants for the application layer.
//!
//! Handlers and services import event types from here so the application layer
//! does not reach directly into `domain::web_ingestion::event_types` at every
//! call site.

pub use crate::domain::web_ingestion::event_types::{aggregate, event};
