//! `QualityChecked` handler (task-book §7.4).
//!
//! Reads the PERSISTED quality decision (never re-runs the gate). Rejected →
//! terminal. Staged/Publishable → advance toward chunking. This handler does
//! NOT publish — publishable only means "permitted to publish later".

use crate::app::web_ingestion::event_types::event as ev;
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::services::{quality_result::QualityResult, terminal_events};
use crate::app::web_ingestion::state_machine_adapter as sm;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repo::{DomainEvent, NewAuditLog};
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

    if is_terminal_run_status(&run.status) {
        return Ok(());
    }
    match run.stage.as_str() {
        run_stage::QUALITY_CHECKED => {} // entry
        run_stage::CHUNKING
        | run_stage::CHUNKED
        | run_stage::EMBEDDING
        | run_stage::EMBEDDED
        | run_stage::INDEXING
        | run_stage::INDEXED
        | run_stage::STAGING
        | run_stage::PUBLISHING => {
            tracing::info!(run_id, stage = %run.stage, "QualityChecked: already past — idempotent");
            return Ok(());
        }
        other => {
            return Err(WebIngestionError::Internal(format!(
                "QualityChecked: unexpected stage '{other}' for run {run_id}"
            )));
        }
    }

    // Read the PERSISTED decision — do NOT recompute the gate (§7.3 #5).
    let quality_json = run.quality_result.as_ref().ok_or_else(|| {
        WebIngestionError::Internal(
            "QualityChecked: quality_result missing — PageDistilled must persist it".into(),
        )
    })?;
    let result = QualityResult::from_json(quality_json);
    tracing::trace!(
        run_id,
        source_id = run.source_id,
        source_url_id = ?run.source_url_id,
        page_id = run.page_id,
        decision = %result.decision,
        reason = %result.reason,
        should_publish = result.should_publish,
        "QualityChecked: persisted quality decision loaded"
    );

    if result.is_rejected() {
        tracing::trace!(
            run_id,
            decision = %result.decision,
            reason = %result.reason,
            "QualityChecked: rejected; emitting terminal rejection"
        );
        let _ = sm::transition(
            &ctx.run_repo,
            run_id,
            run_status::RUNNING,
            run_stage::QUALITY_CHECKED,
            run_status::REJECTED,
            run_stage::REJECTED,
            Some(&result.reason),
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
                message: result.reason.clone(),
                metadata: None,
            })
            .await?;
        terminal_events::emit_rejected(&ctx.outbox_repo, run_id, &run.version_key, &result.reason)
            .await?;
        ctx.run_repo.mark_finished(run_id).await?;
        return Ok(());
    }

    // Staged or publishable → proceed to chunking (NOT publishing).
    if !sm::transition(
        &ctx.run_repo,
        run_id,
        run_status::RUNNING,
        run_stage::QUALITY_CHECKED,
        run_status::RUNNING,
        run_stage::CHUNKING,
        None,
    )
    .await?
    .applied()
    {
        tracing::info!(
            run_id,
            "QualityChecked: not at quality_checked — concurrent worker"
        );
        return Ok(());
    }

    ctx.audit_repo
        .insert(NewAuditLog {
            source_id: Some(run.source_id),
            source_url_id: run.source_url_id,
            page_id: Some(run.page_id),
            run_id: Some(run_id),
            publish_record_id: None,
            action: "quality_passed".into(),
            status: "info".into(),
            message: format!("decision={}, proceeding to chunking", result.decision),
            metadata: None,
        })
        .await?;

    tracing::trace!(
        run_id,
        decision = %result.decision,
        "QualityChecked: passed; emitting DocumentChunked"
    );
    terminal_events::emit_next(
        &ctx.outbox_repo,
        ev::DOCUMENT_CHUNKED,
        run_id,
        &run.version_key,
    )
    .await
}
