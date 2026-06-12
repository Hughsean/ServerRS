//! Crawl scheduler (task-book §0 cron entry).
//!
//! One tick: for every enabled source, create a crawl job and emit a
//! `CrawlJobCreated` event. Discovery of due URLs happens in the handler.

use std::sync::Arc;

use chrono::Utc;

use crate::application::web_ingestion::event_types::{aggregate, event as ev};
use crate::application::web_ingestion::hash;
use crate::domain::web_ingestion::repository::{
    NewOutboxEvent, NewWebCrawlJob, OutboxRepository, WebCrawlJobRepository, WebSourceRepository,
};
use crate::shared::error::AppError;

pub async fn run_tick(
    source_repo: &Arc<dyn WebSourceRepository>,
    crawl_job_repo: &Arc<dyn WebCrawlJobRepository>,
    outbox_repo: &Arc<dyn OutboxRepository>,
    pipeline_version: &str,
) -> Result<(), AppError> {
    let sources = source_repo
        .list_enabled()
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    for source in sources {
        let job = crawl_job_repo
            .insert(NewWebCrawlJob {
                source_id: Some(source.id),
                status: "pending".into(),
                scheduled_at: Utc::now(),
            })
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;

        let event_key = hash::event_key(
            ev::CRAWL_JOB_CREATED,
            aggregate::WEB_CRAWL_JOB,
            job.id,
            0,
            pipeline_version,
        );
        outbox_repo
            .insert_event(NewOutboxEvent {
                event_key,
                event_type: ev::CRAWL_JOB_CREATED.into(),
                aggregate_type: aggregate::WEB_CRAWL_JOB.into(),
                aggregate_id: job.id,
                payload: serde_json::json!({"source_id": source.id, "job_id": job.id}),
                max_retries: 5,
            })
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;
    }
    Ok(())
}
