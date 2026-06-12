//! Outbox dispatcher (task-book §4.3).
//!
//! Responsibilities ONLY: claim a batch of events, route each to its handler,
//! and mark published / failed / dead based on the handler result. No business
//! logic lives here — that is in `handlers/`.

use crate::application::web_ingestion::event_types::event as ev;
use crate::application::web_ingestion::handlers;
use crate::application::web_ingestion::pipeline_context::PipelineContext;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::DomainEvent;
use crate::shared::error::AppError;

/// Run one dispatcher tick: claim a batch and process each event.
pub async fn run_tick(ctx: &PipelineContext) -> Result<(), AppError> {
    let claim_token = format!("dispatcher:{}", uuid::Uuid::new_v4());
    let events = ctx
        .outbox_repo
        .claim_batch(
            &claim_token,
            ctx.config.outbox_lock_ttl_secs,
            ctx.config.outbox_batch_size,
        )
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    for event in events {
        let result = dispatch(&event, ctx).await;
        finalize(ctx, &event, &claim_token, result).await;
    }
    Ok(())
}

/// Route an event to its handler. Unknown types → Err (never marked published).
async fn dispatch(event: &DomainEvent, ctx: &PipelineContext) -> Result<(), WebIngestionError> {
    match event.event_type.as_str() {
        ev::CRAWL_JOB_CREATED => handlers::crawl_job_created::handle(event, ctx).await,
        ev::URL_DISCOVERED => handlers::url_discovered::handle(event, ctx).await,
        ev::PAGE_FETCHED => handlers::page_fetched::handle(event, ctx).await,
        ev::PAGE_CLEANED => handlers::page_cleaned::handle(event, ctx).await,
        ev::PAGE_DISTILLED => handlers::page_distilled::handle(event, ctx).await,
        ev::QUALITY_CHECKED => handlers::quality_checked::handle(event, ctx).await,
        ev::DOCUMENT_CHUNKED => handlers::document_chunked::handle(event, ctx).await,
        ev::CHUNKS_EMBEDDED => handlers::chunks_embedded::handle(event, ctx).await,
        ev::DOCUMENT_INDEXED => handlers::document_indexed::handle(event, ctx).await,
        ev::KNOWLEDGE_STAGED => handlers::knowledge_staged::handle(event, ctx).await,
        ev::KNOWLEDGE_PUBLISH_REQUESTED => handlers::publish_requested::handle(event, ctx).await,
        ev::KNOWLEDGE_ROLLBACK_REQUESTED => handlers::rollback_requested::handle(event, ctx).await,
        // Terminal events — no-op, mark published.
        ev::INGESTION_SKIPPED
        | ev::INGESTION_REJECTED
        | ev::INGESTION_FAILED
        | ev::INGESTION_DEAD
        | ev::KNOWLEDGE_PUBLISHED
        | ev::KNOWLEDGE_SUPERSEDED
        | ev::KNOWLEDGE_ROLLED_BACK => handlers::terminal::handle(event).await,
        other => Err(WebIngestionError::Internal(format!(
            "unsupported outbox event type: {other}"
        ))),
    }
}

/// Apply the handler outcome to the outbox row (claim-token guarded).
async fn finalize(
    ctx: &PipelineContext,
    event: &DomainEvent,
    claim_token: &str,
    result: Result<(), WebIngestionError>,
) {
    match result {
        Ok(()) => match ctx.outbox_repo.mark_published(event.id, claim_token).await {
            Ok(true) => {}
            Ok(false) => tracing::warn!(
                event_id = event.id,
                "mark_published: 0 rows — lock stolen or already processed"
            ),
            Err(e) => tracing::error!(event_id = event.id, error = %e, "mark_published failed"),
        },
        Err(e) => {
            let is_dead = event.retry_count + 1 >= event.max_retries;
            let delay = ctx
                .config
                .retry_base_delay_secs
                .saturating_mul(2u64.saturating_pow(event.retry_count))
                .min(ctx.config.retry_max_delay_secs);
            let next_retry = chrono::Utc::now() + chrono::Duration::seconds(delay as i64);
            match ctx
                .outbox_repo
                .mark_failed_or_dead(event.id, claim_token, &e.to_string(), next_retry, is_dead)
                .await
            {
                Ok(true) => tracing::warn!(
                    event_id = event.id, event_type = %event.event_type, is_dead,
                    error = %e, "event handler failed"
                ),
                Ok(false) => tracing::warn!(
                    event_id = event.id,
                    "mark_failed_or_dead: 0 rows — lock stolen or already processed"
                ),
                Err(mark_err) => tracing::error!(
                    event_id = event.id, handler_error = %e, mark_error = %mark_err,
                    "CRITICAL: handler failed AND mark_failed_or_dead failed — event stuck"
                ),
            }
        }
    }
}

/// Convenience: build a claim token (exposed for tests).
pub fn new_claim_token() -> String {
    format!("dispatcher:{}", uuid::Uuid::new_v4())
}
