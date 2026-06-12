//! `CrawlJobCreated` handler (task-book §5.3).
//!
//! DB job is authoritative. payload.source_id is validated against the DB job
//! and a mismatch FAILS (not just warns). Only due, enabled URLs of an enabled
//! source are discovered.

use crate::application::web_ingestion::event_types::{aggregate, event as ev};
use crate::application::web_ingestion::hash;
use crate::application::web_ingestion::pipeline_context::PipelineContext;
use crate::application::web_ingestion::services::due_url_selector;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::NewOutboxEvent;
use crate::domain::web_ingestion::status::{source_approval, source_trust};
use chrono::Utc;

pub async fn handle(
    event: &crate::domain::web_ingestion::repository::DomainEvent,
    ctx: &PipelineContext,
) -> Result<(), WebIngestionError> {
    let job_id = event.aggregate_id;
    ctx.crawl_job_repo.mark_started(job_id).await?;

    // ── DB job is authoritative (§5.3 CrawlJobCreated) ─────────────────────
    let job = ctx
        .crawl_job_repo
        .find_by_id(job_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "web_crawl_job".into(),
            id: job_id,
        })?;
    let source_id = job.source_id.ok_or_else(|| {
        WebIngestionError::Internal("CrawlJobCreated: crawl_job has no source_id".into())
    })?;

    // payload.source_id mismatch MUST fail (§5.3 #4 — not just warn).
    if let Some(payload_src) = event.payload["source_id"].as_u64() {
        if payload_src != source_id {
            ctx.crawl_job_repo
                .mark_finished(job_id, "failed")
                .await
                .ok();
            return Err(WebIngestionError::Internal(format!(
                "CrawlJobCreated: payload.source_id {payload_src} != DB job.source_id {source_id}"
            )));
        }
    }

    // ── Source must exist, be enabled, approved, and not deleted (§5.3) ────
    let source = ctx
        .source_repo
        .find_by_id(source_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "web_source".into(),
            id: source_id,
        })?;
    if source.deleted_at.is_some() || !source.enabled {
        ctx.crawl_job_repo
            .mark_finished(job_id, "succeeded")
            .await?;
        tracing::info!(
            source_id,
            "CrawlJobCreated: source disabled/deleted — nothing to do"
        );
        return Ok(());
    }
    if source.approval_status == source_approval::REJECTED
        || source.approval_status == source_approval::DISABLED
        || source.trust_level == source_trust::UNTRUSTED
    {
        ctx.crawl_job_repo
            .mark_finished(job_id, "succeeded")
            .await?;
        tracing::info!(source_id, status = %source.approval_status, "CrawlJobCreated: source not approved — skipping");
        return Ok(());
    }

    // ── Discover only due, enabled URLs ────────────────────────────────────
    let now = Utc::now();
    let urls = ctx.source_url_repo.list_by_source(source_id).await?;
    let due = due_url_selector::select_due(urls, now);

    for url in due {
        let event_key = hash::event_key(
            ev::URL_DISCOVERED,
            aggregate::WEB_CRAWL_JOB,
            job_id,
            url.id,
            &url.url_hash,
        );
        ctx.outbox_repo
            .insert_event(NewOutboxEvent {
                event_key,
                event_type: ev::URL_DISCOVERED.into(),
                aggregate_type: aggregate::WEB_CRAWL_JOB.into(),
                aggregate_id: job_id,
                payload: serde_json::json!({
                    "source_url_id": url.id,
                    "source_id": source_id,
                    "url": url.url,
                    "url_hash": url.url_hash,
                    "job_id": job_id
                }),
                max_retries: 5,
            })
            .await?;
    }

    ctx.crawl_job_repo
        .mark_finished(job_id, "succeeded")
        .await?;
    Ok(())
}
