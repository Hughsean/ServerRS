//! Publish / Supersede / Rollback service.
//!
//! Task-book §14 requirements:
//! - Publish lock per (source_id, page_id) via DB transaction + FOR UPDATE
//! - At most one active record per page (enforced by UNIQUE active_page_key)
//! - Publish flow: deactivate old → activate new (DB first, then Qdrant)
//! - Supersede: old version becomes inactive, new version becomes active
//! - Rollback: current → target, rebuilding Qdrant points if needed

use std::sync::Arc;

use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::event_types::{aggregate, event};
use crate::domain::web_ingestion::repository::*;
use crate::domain::web_ingestion::status::*;

/// Publish a staged publish record.
///
/// Returns the new active record id on success.
pub async fn publish(
    publish_record_id: u64,
    publish_repo: &Arc<dyn PublishRecordRepository>,
    chunk_manifest_repo: &Arc<dyn ChunkManifestRepository>,
    vector_manifest_repo: &Arc<dyn VectorManifestRepository>,
    audit_repo: &Arc<dyn AuditLogRepository>,
    outbox_repo: &Arc<dyn OutboxRepository>,
    event_keys: &PublishEventKeys,
) -> Result<(), WebIngestionError> {
    // 1. Load record
    let record = publish_repo
        .find_by_id(publish_record_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "knowledge_publish_record".into(),
            id: publish_record_id,
        })?;

    if record.publish_status != publish_status::STAGED {
        return Err(WebIngestionError::Internal(format!(
            "cannot publish record in {} status",
            record.publish_status
        )));
    }

    // 2. Acquire page lock
    publish_repo
        .lock_page_for_publish(record.source_id, record.page_id)
        .await?;

    // 3. Find old active record
    let old_active = publish_repo
        .find_active_by_page(record.source_id, record.page_id)
        .await?;

    // 4. Deactivate old record (and its manifests)
    if let Some(ref old) = old_active {
        if old.id == record.id {
            // Already the active one — idempotent
            audit_repo
                .insert(NewAuditLog {
                    source_id: Some(record.source_id),
                    page_id: Some(record.page_id),
                    run_id: Some(record.run_id),
                    publish_record_id: Some(record.id),
                    source_url_id: None,
                    action: audit_action::PUBLISH_SUCCEEDED.into(),
                    status: "success".into(),
                    message: "publish idempotent (already active)".into(),
                    metadata: None,
                })
                .await?;
            return Ok(());
        }

        // Deactivate old
        publish_repo
            .set_active(old.id, false, None, publish_status::SUPERSEDED)
            .await?;
        chunk_manifest_repo
            .set_active_by_publish_record(old.id, false)
            .await?;
        vector_manifest_repo
            .set_active_by_publish_record(old.id, false)
            .await?;

        // Insert KnowledgeSuperseded event
        outbox_repo
            .insert_event(NewOutboxEvent {
                event_key: event_keys.superseded_key.clone(),
                event_type: event::KNOWLEDGE_SUPERSEDED.into(),
                aggregate_type: aggregate::KNOWLEDGE_PUBLISH_RECORD.into(),
                aggregate_id: old.id,
                payload: serde_json::json!({
                    "superseded_record_id": old.id,
                    "new_record_id": record.id,
                    "source_id": record.source_id,
                    "page_id": record.page_id
                }),
                max_retries: 5,
            })
            .await?;
    }

    // 5. Activate new record
    let active_key = Some(format!("{}:{}", record.source_id, record.page_id));
    publish_repo
        .set_active(
            record.id,
            true,
            active_key.as_deref(),
            publish_status::PUBLISHED,
        )
        .await?;
    chunk_manifest_repo
        .set_active_by_publish_record(record.id, true)
        .await?;
    vector_manifest_repo
        .set_active_by_publish_record(record.id, true)
        .await?;

    // 6. Insert KnowledgePublished event
    outbox_repo
        .insert_event(NewOutboxEvent {
            event_key: event_keys.published_key.clone(),
            event_type: event::KNOWLEDGE_PUBLISHED.into(),
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

    // 7. Audit
    audit_repo
        .insert(NewAuditLog {
            source_id: Some(record.source_id),
            page_id: Some(record.page_id),
            run_id: Some(record.run_id),
            publish_record_id: Some(record.id),
            source_url_id: None,
            action: audit_action::PUBLISH_SUCCEEDED.into(),
            status: "success".into(),
            message: format!("published record {}", record.id),
            metadata: None,
        })
        .await?;

    Ok(())
}

/// Rollback: restore a previously-superseded target record as the active version.
pub async fn rollback(
    current_record_id: u64,
    target_record_id: u64,
    publish_repo: &Arc<dyn PublishRecordRepository>,
    chunk_manifest_repo: &Arc<dyn ChunkManifestRepository>,
    vector_manifest_repo: &Arc<dyn VectorManifestRepository>,
    audit_repo: &Arc<dyn AuditLogRepository>,
    outbox_repo: &Arc<dyn OutboxRepository>,
) -> Result<(), WebIngestionError> {
    // 1. Load both records
    let current = publish_repo
        .find_by_id(current_record_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "knowledge_publish_record (current)".into(),
            id: current_record_id,
        })?;
    let target = publish_repo
        .find_by_id(target_record_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "knowledge_publish_record (target)".into(),
            id: target_record_id,
        })?;

    // 2. Validate same page
    if current.source_id != target.source_id || current.page_id != target.page_id {
        return Err(WebIngestionError::Internal(
            "rollback: current and target must belong to the same page".into(),
        ));
    }

    // 3. Validate current is active
    if !current.active {
        return Err(WebIngestionError::Internal(
            "rollback: current record is not active".into(),
        ));
    }

    // 4. Acquire page lock
    publish_repo
        .lock_page_for_publish(current.source_id, current.page_id)
        .await?;

    // 5. Deactivate current
    publish_repo
        .set_active(current.id, false, None, publish_status::ROLLED_BACK)
        .await?;
    chunk_manifest_repo
        .set_active_by_publish_record(current.id, false)
        .await?;
    vector_manifest_repo
        .set_active_by_publish_record(current.id, false)
        .await?;

    // 6. Activate target
    let active_key = format!("{}:{}", target.source_id, target.page_id);
    publish_repo
        .set_active(
            target.id,
            true,
            Some(&active_key),
            publish_status::PUBLISHED,
        )
        .await?;
    chunk_manifest_repo
        .set_active_by_publish_record(target.id, true)
        .await?;
    vector_manifest_repo
        .set_active_by_publish_record(target.id, true)
        .await?;

    // 7. Insert KnowledgeRolledBack event
    let event_key = crate::application::web_ingestion::hash::rollback_event_key(
        current_record_id,
        target_record_id,
    );
    outbox_repo
        .insert_event(NewOutboxEvent {
            event_key,
            event_type: event::KNOWLEDGE_ROLLED_BACK.into(),
            aggregate_type: aggregate::KNOWLEDGE_PUBLISH_RECORD.into(),
            aggregate_id: target.id,
            payload: serde_json::json!({
                "rolled_back_from": current_record_id,
                "restored_to": target_record_id,
                "source_id": target.source_id,
                "page_id": target.page_id
            }),
            max_retries: 5,
        })
        .await?;

    // 8. Audit
    audit_repo
        .insert(NewAuditLog {
            source_id: Some(target.source_id),
            page_id: Some(target.page_id),
            run_id: Some(target.run_id),
            publish_record_id: Some(target.id),
            source_url_id: None,
            action: audit_action::ROLLBACK_SUCCEEDED.into(),
            status: "success".into(),
            message: format!(
                "rolled back from {} to {}",
                current_record_id, target_record_id
            ),
            metadata: None,
        })
        .await?;

    Ok(())
}

/// Pre-computed event keys for publish flow (ensures deterministic idempotency).
pub struct PublishEventKeys {
    pub published_key: String,
    pub superseded_key: String,
}
