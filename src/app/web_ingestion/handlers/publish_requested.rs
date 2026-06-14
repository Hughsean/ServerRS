//! `KnowledgePublishRequested` handler (task-book §12.1, §12.2).
//!
//! Transactionally publishes a staged record: supersede the current active
//! version and activate the target, all in ONE DB transaction with a page lock
//! (`publish_in_tx`). Then re-syncs Qdrant `active` payloads (DB is
//! authoritative; Qdrant is best-effort + retryable). Emits KnowledgePublished
//! (+ KnowledgeSuperseded if a prior version was replaced). Idempotent.

use crate::app::web_ingestion::event_types::{aggregate, event as ev};
use crate::app::web_ingestion::hash;
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::services::qdrant_activation_service;
use crate::app::web_ingestion::state_machine_adapter as sm;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::{DomainEvent, NewAuditLog, NewOutboxEvent};
use crate::domain::web_ingestion::status::{audit_action, publish_status, run_stage, run_status};

pub async fn handle(event: &DomainEvent, ctx: &PipelineContext) -> Result<(), WebIngestionError> {
    let publish_record_id = event.payload["publish_record_id"]
        .as_u64()
        .filter(|&v| v > 0)
        .ok_or_else(|| {
            WebIngestionError::Internal(
                "KnowledgePublishRequested: missing/invalid publish_record_id".into(),
            )
        })?;

    let record = ctx
        .publish_repo
        .find_by_id(publish_record_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "knowledge_publish_record".into(),
            id: publish_record_id,
        })?;

    // ── Validate candidate (§12.1 #4): rejected/failed/dead/superseded cannot publish ──
    match record.publish_status.as_str() {
        publish_status::STAGED => {}
        publish_status::PUBLISHED if record.active => {
            reconcile_published_run(ctx, record.run_id).await?;
            tracing::info!(publish_record_id, "publish: already active — idempotent");
            return Ok(());
        }
        other => {
            return Err(WebIngestionError::Internal(format!(
                "publish: record {publish_record_id} in '{other}' status is not publishable"
            )));
        }
    }

    // Rejected runs can never publish. Staged decisions may be published only
    // through an explicit manual request; automatic requests are emitted only
    // for a persisted publishable decision.
    let run = ctx
        .run_repo
        .find_by_id(record.run_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "knowledge_ingestion_run".into(),
            id: record.run_id,
        })?;
    if run.status == crate::domain::web_ingestion::status::run_status::REJECTED {
        return Err(WebIngestionError::Internal(format!(
            "publish: run {} is rejected — cannot publish",
            record.run_id
        )));
    }

    if run.status == run_status::STAGED && run.stage == run_stage::STAGING {
        if !sm::transition(
            &ctx.run_repo,
            run.id,
            run_status::STAGED,
            run_stage::STAGING,
            run_status::RUNNING,
            run_stage::PUBLISHING,
            None,
        )
        .await?
        .applied()
        {
            tracing::info!(run_id = run.id, "publish: run state changed concurrently");
            return Ok(());
        }
    } else if !(run.status == run_status::RUNNING && run.stage == run_stage::PUBLISHING) {
        return Err(WebIngestionError::Internal(format!(
            "publish: run {} is in unexpected state ({},{})",
            run.id, run.status, run.stage
        )));
    }

    ctx.audit_repo
        .insert(NewAuditLog {
            source_id: Some(record.source_id),
            page_id: Some(record.page_id),
            run_id: Some(record.run_id),
            publish_record_id: Some(record.id),
            source_url_id: None,
            action: audit_action::PUBLISH_STARTED.into(),
            status: "info".into(),
            message: format!("publish requested for record {}", record.id),
            metadata: None,
        })
        .await?;

    // ── Atomic DB publish (lock + supersede old + activate new) ────────────
    let outcome = ctx.publish_repo.publish_in_tx(publish_record_id).await?;

    if outcome.was_already_active {
        reconcile_published_run(ctx, record.run_id).await?;
        tracing::info!(publish_record_id, "publish: idempotent (already active)");
        return Ok(());
    }

    let dimension = ctx.embedding_dimension();

    // ── Qdrant re-sync: deactivate old, activate new ───────────────────────
    // DB is already committed & authoritative. A Qdrant failure here is logged
    // and audited but does NOT roll back the DB (RetrievalService re-validates
    // against DB status, so stale Qdrant active flags cannot leak content).
    if let Some(old_record_id) = outcome.deactivated_record_id {
        sync_qdrant(ctx, old_record_id, dimension, false, "supersede").await;
        // Emit KnowledgeSuperseded for the old record.
        let event_key = hash::event_key(
            ev::KNOWLEDGE_SUPERSEDED,
            aggregate::KNOWLEDGE_PUBLISH_RECORD,
            old_record_id,
            record.id,
            &record.version_key,
        );
        ctx.outbox_repo
            .insert_event(NewOutboxEvent {
                event_key,
                event_type: ev::KNOWLEDGE_SUPERSEDED.into(),
                aggregate_type: aggregate::KNOWLEDGE_PUBLISH_RECORD.into(),
                aggregate_id: old_record_id,
                payload: serde_json::json!({
                    "superseded_record_id": old_record_id,
                    "new_record_id": record.id,
                    "source_id": record.source_id,
                    "page_id": record.page_id
                }),
                max_retries: 5,
            })
            .await?;
    }
    sync_qdrant(ctx, record.id, dimension, true, "publish").await;

    reconcile_published_run(ctx, record.run_id).await?;

    ctx.audit_repo
        .insert(NewAuditLog {
            source_id: Some(record.source_id),
            page_id: Some(record.page_id),
            run_id: Some(record.run_id),
            publish_record_id: Some(record.id),
            source_url_id: None,
            action: audit_action::PUBLISH_SUCCEEDED.into(),
            status: "success".into(),
            message: format!(
                "published record {} (document {}); superseded {:?}",
                record.id, outcome.activated_document_id, outcome.deactivated_record_id
            ),
            metadata: None,
        })
        .await?;

    // Emit KnowledgePublished (terminal).
    let event_key = hash::event_key(
        ev::KNOWLEDGE_PUBLISHED,
        aggregate::KNOWLEDGE_PUBLISH_RECORD,
        record.id,
        record.run_id,
        &record.version_key,
    );
    ctx.outbox_repo
        .insert_event(NewOutboxEvent {
            event_key,
            event_type: ev::KNOWLEDGE_PUBLISHED.into(),
            aggregate_type: aggregate::KNOWLEDGE_PUBLISH_RECORD.into(),
            aggregate_id: record.id,
            payload: serde_json::json!({
                "publish_record_id": record.id,
                "run_id": record.run_id,
                "source_id": record.source_id,
                "page_id": record.page_id
            }),
            max_retries: 5,
        })
        .await?;

    Ok(())
}

async fn reconcile_published_run(
    ctx: &PipelineContext,
    run_id: u64,
) -> Result<(), WebIngestionError> {
    let run =
        ctx.run_repo
            .find_by_id(run_id)
            .await?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_ingestion_run".into(),
                id: run_id,
            })?;

    if run.status == run_status::PUBLISHED && run.stage == run_stage::PUBLISHED {
        return Ok(());
    }

    if run.status == run_status::STAGED && run.stage == run_stage::STAGING {
        let _ = sm::transition(
            &ctx.run_repo,
            run_id,
            run_status::STAGED,
            run_stage::STAGING,
            run_status::RUNNING,
            run_stage::PUBLISHING,
            None,
        )
        .await?;
    }

    let _ = sm::transition(
        &ctx.run_repo,
        run_id,
        run_status::RUNNING,
        run_stage::PUBLISHING,
        run_status::PUBLISHED,
        run_stage::PUBLISHED,
        None,
    )
    .await?;
    ctx.run_repo.mark_finished(run_id).await
}

/// Re-sync a publish record's Qdrant points to `active`. Failures are audited
/// (status=qdrant_cleanup_failed) but do not fail the publish — the DB is
/// authoritative and retrieval re-validates against it.
async fn sync_qdrant(
    ctx: &PipelineContext,
    publish_record_id: u64,
    dimension: usize,
    active: bool,
    phase: &str,
) {
    if let Err(e) = qdrant_activation_service::sync_active(
        &ctx.vector_store,
        &ctx.vector_manifest_repo,
        &ctx.rag_repo,
        publish_record_id,
        dimension,
        active,
    )
    .await
    {
        tracing::error!(
            publish_record_id, phase, error = %e,
            "qdrant active re-sync failed — DB is authoritative; will need reconciliation"
        );
        let _ = ctx
            .audit_repo
            .insert(NewAuditLog {
                source_id: None,
                page_id: None,
                run_id: None,
                publish_record_id: Some(publish_record_id),
                source_url_id: None,
                action: audit_action::QDRANT_CLEANUP_FAILED.into(),
                status: "error".into(),
                message: format!("qdrant {phase} active={active} sync failed: {e}"),
                metadata: None,
            })
            .await;
    }
}
