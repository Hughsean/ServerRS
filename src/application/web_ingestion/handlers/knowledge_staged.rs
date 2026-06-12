//! `KnowledgeStaged` handler (task-book §11).
//!
//! Confirms the run's chunks / embeddings / vector points are all staged, marks
//! the run `staged/staging`, and writes a publish-candidate audit record. It
//! does NOT publish — publishing is a separate, manual `KnowledgePublishRequested`
//! event (§11 #1). Idempotent + resumable per §5.8.

use crate::application::web_ingestion::pipeline_context::PipelineContext;
use crate::application::web_ingestion::services::quality_result::QualityResult;
use crate::application::web_ingestion::state_machine_adapter as sm;
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
    // Already staged (staged/staging) or publishing → idempotent.
    if run.status == run_status::STAGED || run.stage == run_stage::PUBLISHING {
        tracing::info!(run_id, "KnowledgeStaged: already staged — idempotent");
        return Ok(());
    }
    if run.stage != run_stage::INDEXED {
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

    // Determine whether this run is a publish candidate. high-risk content is
    // never auto-publishable; this only RECORDS the candidate — it does not
    // publish. With auto_publish off, should_publish is always false (§11).
    let publishable = run
        .quality_result
        .as_ref()
        .map(|qr| QualityResult::from_json(qr).is_publishable())
        .unwrap_or(false);

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
                "run staged (publish_record={}); publishable={publishable}; awaiting manual publish",
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

    tracing::info!(
        run_id,
        publish_record_id = publish_record.id,
        publishable,
        "KnowledgeStaged: run staged — NOT auto-published"
    );

    // No downstream event — publish is a separate manual request (§11 #1).
    Ok(())
}
