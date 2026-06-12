//! Terminal-event emission helpers (task-book §4.8, §14).
//!
//! Emits the small, idempotent outbox events that mark the end of a pipeline
//! branch (rejected / skipped). Payloads carry only ids + small metadata.

use std::sync::Arc;

use crate::application::web_ingestion::event_types::{aggregate, event as ev};
use crate::application::web_ingestion::hash;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::{NewOutboxEvent, OutboxRepository};

/// Emit an `IngestionRejected` terminal event for a run.
pub async fn emit_rejected(
    outbox_repo: &Arc<dyn OutboxRepository>,
    run_id: u64,
    version_key: &str,
    reason: &str,
) -> Result<(), WebIngestionError> {
    let event_key = hash::event_key(
        ev::INGESTION_REJECTED,
        aggregate::KNOWLEDGE_INGESTION_RUN,
        run_id,
        run_id,
        version_key,
    );
    outbox_repo
        .insert_event(NewOutboxEvent {
            event_key,
            event_type: ev::INGESTION_REJECTED.into(),
            aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
            aggregate_id: run_id,
            payload: serde_json::json!({"run_id": run_id, "reason": reason}),
            max_retries: 3,
        })
        .await?;
    Ok(())
}

/// Emit an `IngestionSkipped` terminal event (content unchanged etc.).
pub async fn emit_skipped(
    outbox_repo: &Arc<dyn OutboxRepository>,
    source_url_id: u64,
    url_hash: &str,
    reason: &str,
) -> Result<(), WebIngestionError> {
    let event_key = hash::event_key(
        ev::INGESTION_SKIPPED,
        aggregate::KNOWLEDGE_INGESTION_RUN,
        0,
        source_url_id,
        url_hash,
    );
    outbox_repo
        .insert_event(NewOutboxEvent {
            event_key,
            event_type: ev::INGESTION_SKIPPED.into(),
            aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
            aggregate_id: 0,
            payload: serde_json::json!({"source_url_id": source_url_id, "reason": reason}),
            max_retries: 3,
        })
        .await?;
    Ok(())
}

/// Emit a generic next-stage event with a small payload.
pub async fn emit_next(
    outbox_repo: &Arc<dyn OutboxRepository>,
    event_type: &str,
    run_id: u64,
    version_key: &str,
) -> Result<(), WebIngestionError> {
    let event_key = hash::event_key(
        event_type,
        aggregate::KNOWLEDGE_INGESTION_RUN,
        run_id,
        run_id,
        version_key,
    );
    outbox_repo
        .insert_event(NewOutboxEvent {
            event_key,
            event_type: event_type.into(),
            aggregate_type: aggregate::KNOWLEDGE_INGESTION_RUN.into(),
            aggregate_id: run_id,
            payload: serde_json::json!({"run_id": run_id}),
            max_retries: 5,
        })
        .await?;
    Ok(())
}
