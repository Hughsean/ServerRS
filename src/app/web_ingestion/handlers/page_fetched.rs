//! `PageFetched` handler (task-book §7.1).
//!
//! Reads the raw fetched body from the artifact column (NOT the outbox
//! payload), cleans it, rejects too-short pages before any LLM call, and emits
//! `PageCleaned`. Idempotent + resumable per §5.8.

use crate::app::web_ingestion::event_types::event as ev;
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::services::{artifact_service, html_cleaner, terminal_events};
use crate::app::web_ingestion::state_machine_adapter as sm;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::{DomainEvent, NewAuditLog};
use crate::domain::web_ingestion::status::{
    audit_action, is_terminal_run_status, run_stage, run_status,
};

pub async fn handle(event: &DomainEvent, ctx: &PipelineContext) -> Result<(), WebIngestionError> {
    let run_id = event.aggregate_id;
    let run =
        ctx.run_repo
            .find_by_id(run_id)
            .await?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_ingestion_run".into(),
                id: run_id,
            })?;

    // ── Resume classification (§5.8) ───────────────────────────────────────
    if is_terminal_run_status(&run.status) {
        return Ok(()); // terminal — branch ended
    }
    match run.stage.as_str() {
        // Entry: clean from the fetched body.
        run_stage::FETCHED => {}
        // Mid: cleaning was started but not finished — clean text may be absent.
        run_stage::CLEANING => {}
        // Already cleaned or later — idempotent success.
        run_stage::CLEANED
        | run_stage::DISTILLING
        | run_stage::DISTILLED
        | run_stage::QUALITY_CHECKED
        | run_stage::CHUNKING
        | run_stage::CHUNKED
        | run_stage::EMBEDDING
        | run_stage::EMBEDDED
        | run_stage::INDEXING
        | run_stage::INDEXED
        | run_stage::STAGING
        | run_stage::PUBLISHING => {
            tracing::info!(run_id, stage = %run.stage, "PageFetched: already past — idempotent");
            return Ok(());
        }
        // Too early (fetching/pending) or impossible — fail so it is retried.
        other => {
            return Err(WebIngestionError::Internal(format!(
                "PageFetched: unexpected stage '{other}' for run {run_id}"
            )));
        }
    }

    let body = run.fetched_body_text.as_deref().ok_or_else(|| {
        WebIngestionError::Internal(
            "PageFetched: fetched_body_text missing — artifact not persisted".into(),
        )
    })?;

    let (_title, clean_text) = html_cleaner::clean(body);
    let raw_chars = body.chars().count();
    let clean_chars = clean_text.chars().count();
    tracing::debug!(
        run_id,
        source_id = run.source_id,
        source_url_id = ?run.source_url_id,
        page_id = run.page_id,
        raw_chars,
        clean_chars,
        "PageFetched: html cleaned"
    );

    // running/fetched → running/cleaning (only when entering at fetched).
    if run.stage == run_stage::FETCHED
        && !sm::transition(
            &ctx.run_repo,
            run_id,
            run_status::RUNNING,
            run_stage::FETCHED,
            run_status::RUNNING,
            run_stage::CLEANING,
            None,
        )
        .await?
        .applied()
    {
        tracing::info!(run_id, "PageFetched: not at fetched — concurrent worker");
        return Ok(());
    }

    // Short-text reject (no LLM call). cleaning → rejected.
    if html_cleaner::is_too_short(&clean_text) {
        let _ = sm::transition(
            &ctx.run_repo,
            run_id,
            run_status::RUNNING,
            run_stage::CLEANING,
            run_status::REJECTED,
            run_stage::REJECTED,
            Some("clean_text too short (< 100 chars)"),
        )
        .await?;
        ctx.audit_repo
            .insert(NewAuditLog {
                source_id: Some(run.source_id),
                source_url_id: run.source_url_id,
                page_id: Some(run.page_id),
                run_id: Some(run_id),
                publish_record_id: None,
                action: audit_action::QUALITY_REJECTED.into(),
                status: "rejected".into(),
                message: "clean_text too short".into(),
                metadata: None,
            })
            .await?;
        terminal_events::emit_rejected(&ctx.outbox_repo, run_id, &run.version_key, "too_short")
            .await?;
        ctx.run_repo.mark_finished(run_id).await?;
        return Ok(());
    }

    artifact_service::save_clean_text(&ctx.run_repo, run_id, &clean_text).await?;

    // running/cleaning → running/cleaned
    let _ = sm::transition(
        &ctx.run_repo,
        run_id,
        run_status::RUNNING,
        run_stage::CLEANING,
        run_status::RUNNING,
        run_stage::CLEANED,
        None,
    )
    .await?;

    tracing::debug!(
        run_id,
        page_id = run.page_id,
        clean_chars,
        "PageFetched: clean text persisted; emitting PageCleaned"
    );

    terminal_events::emit_next(&ctx.outbox_repo, ev::PAGE_CLEANED, run_id, &run.version_key).await
}
