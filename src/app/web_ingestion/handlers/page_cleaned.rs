//! `PageCleaned` handler (task-book §7.2).
//!
//! Calls the distill LLM with the cleaned text as UNTRUSTED user content (never
//! system prompt). Validates structured JSON, persists distilled_json + usage,
//! and emits `PageDistilled`. Idempotent + resumable per §5.8.

use crate::app::web_ingestion::event_types::event as ev;
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::services::{artifact_service, terminal_events};
use crate::app::web_ingestion::state_machine_adapter as sm;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::{DomainEvent, NewAuditLog};
use crate::domain::web_ingestion::status::{is_terminal_run_status, run_stage, run_status};

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
        run_stage::CLEANED | run_stage::DISTILLING => {} // entry or mid (resume)
        run_stage::DISTILLED
        | run_stage::QUALITY_CHECKED
        | run_stage::CHUNKING
        | run_stage::CHUNKED
        | run_stage::EMBEDDING
        | run_stage::EMBEDDED
        | run_stage::INDEXING
        | run_stage::INDEXED
        | run_stage::STAGING
        | run_stage::PUBLISHING => {
            tracing::info!(run_id, stage = %run.stage, "PageCleaned: already past — idempotent");
            return Ok(());
        }
        other => {
            return Err(WebIngestionError::Internal(format!(
                "PageCleaned: unexpected stage '{other}' for run {run_id}"
            )));
        }
    }

    let clean_text = run
        .clean_text
        .as_deref()
        .ok_or_else(|| WebIngestionError::Internal("PageCleaned: clean_text missing".into()))?
        .to_string();

    // running/cleaned → running/distilling (only when entering at cleaned).
    if run.stage == run_stage::CLEANED
        && !sm::transition(
            &ctx.run_repo,
            run_id,
            run_status::RUNNING,
            run_stage::CLEANED,
            run_status::RUNNING,
            run_stage::DISTILLING,
            None,
        )
        .await?
        .applied()
    {
        tracing::info!(run_id, "PageCleaned: not at cleaned — concurrent worker");
        return Ok(());
    }

    // Untrusted content goes in as user data; the infrastructure adapter
    // enforces prompt-injection guards.
    let page = ctx
        .page_repo
        .find_by_id(run.page_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "web_page".into(),
            id: run.page_id,
        })?;
    let url = page.canonical_url.as_deref().unwrap_or(&page.url);
    let distill_result = match ctx.distiller.distill(&clean_text, &url).await {
        Ok(r) => r,
        Err(WebIngestionError::DistillJsonParseFailed { error }) => {
            // Retry already happened inside distill(); failing now means the
            // model could not produce valid JSON. Do NOT fake success — fail.
            ctx.audit_repo
                .insert(NewAuditLog {
                    source_id: Some(run.source_id),
                    source_url_id: run.source_url_id,
                    page_id: Some(run.page_id),
                    run_id: Some(run_id),
                    publish_record_id: None,
                    action: "distill_failed".into(),
                    status: "error".into(),
                    message: format!("distill JSON parse failed after retry: {error}"),
                    metadata: None,
                })
                .await?;
            return Err(WebIngestionError::DistillJsonParseFailed { error });
        }
        Err(e) => return Err(e),
    };

    let distilled_value = serde_json::to_value(&distill_result.distilled)
        .map_err(|e| WebIngestionError::Internal(format!("serialize distilled: {e}")))?;
    artifact_service::save_distilled(&ctx.run_repo, run_id, distilled_value).await?;

    ctx.run_repo
        .update_distill_result(
            run_id,
            &ctx.config.distill_llm.provider,
            &ctx.config.distill_llm.chat_model,
            ctx.llm_prompt_version(),
            distill_result.llm_input_tokens,
            distill_result.llm_output_tokens,
            distill_result.distilled.quality_score,
            // quality_result is computed by PageDistilled; placeholder for now.
            serde_json::json!({}),
            serde_json::json!(distill_result.distilled.risk_flags),
            distill_result.distilled.should_publish,
        )
        .await?;

    // running/distilling → running/distilled
    let _ = sm::transition(
        &ctx.run_repo,
        run_id,
        run_status::RUNNING,
        run_stage::DISTILLING,
        run_status::RUNNING,
        run_stage::DISTILLED,
        None,
    )
    .await?;

    terminal_events::emit_next(
        &ctx.outbox_repo,
        ev::PAGE_DISTILLED,
        run_id,
        &run.version_key,
    )
    .await
}
