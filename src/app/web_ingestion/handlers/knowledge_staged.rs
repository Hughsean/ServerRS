//! `KnowledgeStaged` handler (task-book §11).
//!
//! Confirms the run's chunks / embeddings / vector points are all staged, marks
//! the run `staged/staging`, and writes a publish-candidate audit record.
//! Publishable runs emit an idempotent automatic publish request when the
//! global switch is enabled; all other runs remain staged for manual review.

use crate::app::web_ingestion::event_types::{aggregate, event as ev};
use crate::app::web_ingestion::hash;
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::services::quality_result::QualityResult;
use crate::app::web_ingestion::state_machine_adapter as sm;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::{DomainEvent, NewAuditLog, NewOutboxEvent};
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
    if run.stage == run_stage::PUBLISHING {
        tracing::info!(run_id, "KnowledgeStaged: already publishing — idempotent");
        return Ok(());
    }
    let already_staged = run.status == run_status::STAGED && run.stage == run_stage::STAGING;
    if !already_staged && run.stage != run_stage::INDEXED {
        return Err(WebIngestionError::Internal(format!(
            "KnowledgeStaged: unexpected stage '{}' for run {run_id}",
            run.stage
        )));
    }

    // Verify the staged artifacts exist before declaring the run staged.
    let publish_record = ctx
        .publish_repo
        .find_by_run_id(run_id)
        .await?
        .ok_or_else(|| {
            WebIngestionError::Internal("KnowledgeStaged: publish record missing".into())
        })?;
    let chunk_manifests = ctx
        .chunk_manifest_repo
        .list_by_publish_record(publish_record.id)
        .await?;
    let vector_manifests = ctx
        .vector_manifest_repo
        .list_by_publish_record(publish_record.id)
        .await?;
    if chunk_manifests.is_empty() || vector_manifests.is_empty() {
        return Err(WebIngestionError::Internal(
            "KnowledgeStaged: chunk/vector manifest missing — cannot stage".into(),
        ));
    }

    // indexed → staged/staging
    if !already_staged {
        if !sm::transition(
            &ctx.run_repo,
            run_id,
            run_status::RUNNING,
            run_stage::INDEXED,
            run_status::STAGED,
            run_stage::STAGING,
            None,
        )
        .await?
        .applied()
        {
            tracing::info!(
                run_id,
                "KnowledgeStaged: not at indexed — concurrent worker"
            );
            return Ok(());
        }
    }

    // Determine whether this run is a publish candidate. High-risk content and
    // sources that require review are never auto-publishable.
    let publishable = run
        .quality_result
        .as_ref()
        .map(|qr| QualityResult::from_json(qr).is_publishable())
        .unwrap_or(false);

    if !already_staged {
        ctx.audit_repo
            .insert(NewAuditLog {
                source_id: Some(run.source_id),
                source_url_id: run.source_url_id,
                page_id: Some(run.page_id),
                run_id: Some(run_id),
                publish_record_id: Some(publish_record.id),
                action: if publishable {
                    "publish_candidate"
                } else {
                    "staged_manual_review"
                }
                .into(),
                status: "staged".into(),
                message: format!(
                    "run staged (publish_record={}); publishable={publishable}",
                    publish_record.id
                ),
                metadata: Some(serde_json::json!({
                    "publish_record_id": publish_record.id,
                    "chunk_count": chunk_manifests.len(),
                    "vector_count": vector_manifests.len(),
                    "publishable": publishable,
                    "auto_publish": ctx.config.auto_publish,
                })),
            })
            .await?;
    }

    let auto_publish_requested = should_request_auto_publish(ctx.config.auto_publish, &run);
    tracing::info!(
        run_id,
        publish_record_id = publish_record.id,
        publishable,
        auto_publish_requested,
        "KnowledgeStaged: run staged"
    );

    if auto_publish_requested {
        let event_key = hash::event_key(
            ev::KNOWLEDGE_PUBLISH_REQUESTED,
            aggregate::KNOWLEDGE_PUBLISH_RECORD,
            publish_record.id,
            run_id,
            &run.version_key,
        );
        ctx.outbox_repo
            .insert_event(NewOutboxEvent {
                event_key,
                event_type: ev::KNOWLEDGE_PUBLISH_REQUESTED.into(),
                aggregate_type: aggregate::KNOWLEDGE_PUBLISH_RECORD.into(),
                aggregate_id: publish_record.id,
                payload: serde_json::json!({
                    "publish_record_id": publish_record.id,
                    "run_id": run_id,
                    "automatic": true
                }),
                max_retries: 5,
            })
            .await?;
        tracing::info!(
            run_id,
            publish_record_id = publish_record.id,
            "KnowledgeStaged: automatic publish requested"
        );
    }

    Ok(())
}

fn should_request_auto_publish(
    global_auto_publish: bool,
    run: &crate::domain::web_ingestion::repository::KnowledgeIngestionRun,
) -> bool {
    global_auto_publish
        && run
            .quality_result
            .as_ref()
            .map(|value| QualityResult::from_json(value).is_publishable())
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::web_ingestion::services::quality_result::decision;
    use chrono::Utc;

    fn run_with_decision(
        value: &str,
    ) -> crate::domain::web_ingestion::repository::KnowledgeIngestionRun {
        crate::domain::web_ingestion::repository::KnowledgeIngestionRun {
            id: 1,
            source_id: 1,
            source_url_id: Some(1),
            crawl_job_id: Some(1),
            page_id: 1,
            content_hash: "content".into(),
            content_key: "content-key".into(),
            run_key: "run-key".into(),
            version_key: "version-key".into(),
            status: run_status::STAGED.into(),
            stage: run_stage::STAGING.into(),
            llm_provider: None,
            llm_model: None,
            llm_prompt_version: None,
            llm_input_tokens: None,
            llm_output_tokens: None,
            chunker_version: None,
            embedding_provider: None,
            embedding_model: None,
            embedding_dimension: None,
            quality_score: Some(0.9),
            quality_result: Some(serde_json::json!({
                "decision": value,
                "reason": "",
                "quality_score": 0.9,
                "risk_flags": [],
                "should_publish": value == decision::PUBLISHABLE,
                "gate_version": "test",
                "evaluated_at": Utc::now().to_rfc3339()
            })),
            risk_flags: None,
            should_publish: Some(value == decision::PUBLISHABLE),
            last_error: None,
            retry_count: 0,
            started_at: None,
            finished_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            fetched_body_text: None,
            clean_text: None,
            distilled_json: None,
        }
    }

    #[test]
    fn auto_publish_requires_global_switch_and_publishable_decision() {
        assert!(should_request_auto_publish(
            true,
            &run_with_decision(decision::PUBLISHABLE)
        ));
        assert!(!should_request_auto_publish(
            false,
            &run_with_decision(decision::PUBLISHABLE)
        ));
        assert!(!should_request_auto_publish(
            true,
            &run_with_decision(decision::STAGED)
        ));
    }
}
