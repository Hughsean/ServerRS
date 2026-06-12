//! Terminal-event handler (task-book §4.8, §14.1 #7).
//!
//! Terminal events (IngestionSkipped/Rejected/Failed/Dead and the publish
//! lifecycle events KnowledgePublished/Superseded/RolledBack) signal the end of
//! a branch and require no further work. They are no-op + marked published.

use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::DomainEvent;

pub async fn handle(event: &DomainEvent) -> Result<(), WebIngestionError> {
    tracing::info!(
        event_id = event.id,
        event_type = %event.event_type,
        "terminal event — marking published"
    );
    Ok(())
}
