//! `PageDistilled` handler (task-book §7.3).
//!
//! Runs the quality gate ONCE here and persists a stable, machine-readable
//! `quality_result` (no Rust Debug strings). The global `auto_publish` master
//! switch (§5.1) downgrades any Publishable verdict to Staged. Emits
//! `QualityChecked`. Idempotent + resumable per §5.8.

use crate::app::web_ingestion::event_types::event as ev;
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::quality_gate::{self, QualityGateDecision};
use crate::app::web_ingestion::services::{quality_result::QualityResult, terminal_events};
use crate::app::web_ingestion::state_machine_adapter as sm;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::DomainEvent;
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
        run_stage::DISTILLED => {} // entry
        run_stage::QUALITY_CHECKED
        | run_stage::CHUNKING
        | run_stage::CHUNKED
        | run_stage::EMBEDDING
        | run_stage::EMBEDDED
        | run_stage::INDEXING
        | run_stage::INDEXED
        | run_stage::STAGING
        | run_stage::PUBLISHING => {
            tracing::info!(run_id, stage = %run.stage, "PageDistilled: already past — idempotent");
            return Ok(());
        }
        other => {
            return Err(WebIngestionError::Internal(format!(
                "PageDistilled: unexpected stage '{other}' for run {run_id}"
            )));
        }
    }

    let source = ctx
        .source_repo
        .find_by_id(run.source_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "web_source".into(),
            id: run.source_id,
        })?;

    let distilled = run.distilled_json.as_ref().ok_or_else(|| {
        WebIngestionError::Internal("PageDistilled: distilled_json missing".into())
    })?;

    let sections_count = distilled["sections"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    let summary = distilled["summary"].as_str().unwrap_or("");
    let risk_flags: Vec<String> = distilled["risk_flags"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let quality_score = distilled["quality_score"].as_f64().unwrap_or(0.0);

    let gate_input = quality_gate::QualityGateInput {
        clean_text: run.clean_text.clone().unwrap_or_default(),
        distilled_accept: distilled["accept"].as_bool().unwrap_or(false),
        distilled_summary: summary.to_string(),
        distilled_sections_count: sections_count,
        distilled_quality_score: quality_score,
        distilled_risk_flags: risk_flags.clone(),
        source_approval_status: source.approval_status,
        source_auto_publish: source.auto_publish,
        source_trust_level: source.trust_level,
        // staging_required OR global auto_publish off → force staging.
        staging_required: ctx.config.staging_required || !ctx.config.auto_publish,
        auto_publish_min_score: ctx.config.auto_publish_min_score,
    };
    let mut decision = quality_gate::evaluate(&gate_input)?;

    // Master switch defence-in-depth: never publishable when auto_publish is off.
    if !ctx.config.auto_publish {
        if let QualityGateDecision::Publishable = decision {
            decision = QualityGateDecision::Staged {
                reason: "auto_publish master switch is off".into(),
            };
        }
    }

    let result = QualityResult::from_decision(&decision, quality_score, risk_flags.clone());
    tracing::debug!(
        run_id,
        source_id = run.source_id,
        source_url_id = ?run.source_url_id,
        page_id = run.page_id,
        quality_score,
        decision = %result.decision,
        should_publish = result.should_publish,
        sections = sections_count,
        risk_flags = risk_flags.len(),
        reason = %result.reason,
        "PageDistilled: quality gate evaluated"
    );

    // Persist the STABLE quality_result (§7.3).
    ctx.run_repo
        .update_distill_result(
            run_id,
            &ctx.config.distill_llm.provider,
            &ctx.config.distill_llm.chat_model,
            ctx.llm_prompt_version(),
            run.llm_input_tokens,
            run.llm_output_tokens,
            quality_score,
            result.to_json(),
            serde_json::json!(risk_flags),
            result.should_publish,
        )
        .await?;

    // running/distilled → running/quality_checked
    let _ = sm::transition(
        &ctx.run_repo,
        run_id,
        run_status::RUNNING,
        run_stage::DISTILLED,
        run_status::RUNNING,
        run_stage::QUALITY_CHECKED,
        None,
    )
    .await?;

    tracing::debug!(
        run_id,
        page_id = run.page_id,
        decision = %result.decision,
        "PageDistilled: quality result persisted; emitting QualityChecked"
    );

    terminal_events::emit_next(
        &ctx.outbox_repo,
        ev::QUALITY_CHECKED,
        run_id,
        &run.version_key,
    )
    .await
}
