//! `KnowledgeRollbackRequested` handler (task-book §12.3).
//!
//! Transactionally rolls the active version back to a previous one
//! (`rollback_in_tx`: deactivate current, reactivate target, in ONE tx with a
//! page lock). Then re-syncs Qdrant `active` payloads. Emits KnowledgeRolledBack.
//! Idempotent.

use crate::app::web_ingestion::event_types::{aggregate, event as ev};
use crate::app::web_ingestion::hash;
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::services::qdrant_activation_service;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::{DomainEvent, NewAuditLog, NewOutboxEvent};
use crate::domain::web_ingestion::status::audit_action;

pub async fn handle(event: &DomainEvent, ctx: &PipelineContext) -> Result<(), WebIngestionError> {
    let current_record_id = event.payload["current_record_id"]
        .as_u64()
        .filter(|&v| v > 0)
        .ok_or_else(|| {
            WebIngestionError::Internal(
                "KnowledgeRollbackRequested: missing/invalid current_record_id".into(),
            )
        })?;
    let target_record_id = event.payload["target_record_id"]
        .as_u64()
        .filter(|&v| v > 0)
        .ok_or_else(|| {
            WebIngestionError::Internal(
                "KnowledgeRollbackRequested: missing/invalid target_record_id".into(),
            )
        })?;

    // Idempotency: if the target is already the active record, nothing to do.
    if let Some(target) = ctx.publish_repo.find_by_id(target_record_id).await? {
        if target.active {
            tracing::info!(
                target_record_id,
                "rollback: target already active — idempotent"
            );
            return Ok(());
        }
    }

    ctx.audit_repo
        .insert(NewAuditLog {
            source_id: None,
            page_id: None,
            run_id: None,
            publish_record_id: Some(current_record_id),
            source_url_id: None,
            action: audit_action::ROLLBACK_STARTED.into(),
            status: "info".into(),
            message: format!("rollback requested: {current_record_id} → {target_record_id}"),
            metadata: None,
        })
        .await?;

    // ── Atomic DB rollback ─────────────────────────────────────────────────
    let outcome = ctx
        .publish_repo
        .rollback_in_tx(current_record_id, target_record_id)
        .await?;

    let dimension = ctx.embedding_dimension();

    // Deactivate current in Qdrant, reactivate target.
    if let Some(deactivated) = outcome.deactivated_record_id {
        sync_qdrant(ctx, deactivated, dimension, false, "rollback_deactivate").await;
    }
    sync_qdrant(
        ctx,
        outcome.activated_record_id,
        dimension,
        true,
        "rollback_activate",
    )
    .await;

    ctx.audit_repo
        .insert(NewAuditLog {
            source_id: None,
            page_id: None,
            run_id: None,
            publish_record_id: Some(outcome.activated_record_id),
            source_url_id: None,
            action: audit_action::ROLLBACK_SUCCEEDED.into(),
            status: "success".into(),
            message: format!("rolled back from {current_record_id} to {target_record_id}"),
            metadata: None,
        })
        .await?;

    let event_key = hash::rollback_event_key(current_record_id, target_record_id);
    ctx.outbox_repo
        .insert_event(NewOutboxEvent {
            event_key,
            event_type: ev::KNOWLEDGE_ROLLED_BACK.into(),
            aggregate_type: aggregate::KNOWLEDGE_PUBLISH_RECORD.into(),
            aggregate_id: target_record_id,
            payload: serde_json::json!({
                "rolled_back_from": current_record_id,
                "restored_to": target_record_id,
            }),
            max_retries: 5,
        })
        .await?;

    Ok(())
}

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
            "qdrant rollback re-sync failed — DB is authoritative; will need reconciliation"
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
