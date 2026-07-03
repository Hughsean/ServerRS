//! Placeholder handlers for pipeline stages not yet implemented.
//!
//! Task-book §14.1 #6: a known-but-unimplemented event MUST NOT be marked
//! published. Returning `Err` keeps the event in the outbox (retry/dead) so no
//! progress is silently lost. These are replaced as each phase lands.

use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repo::DomainEvent;

pub async fn not_implemented(event: &DomainEvent, stage: &str) -> Result<(), WebIngestionError> {
    Err(WebIngestionError::Internal(format!(
        "handler {stage} not yet implemented (event_id={}, type={})",
        event.id, event.event_type
    )))
}
