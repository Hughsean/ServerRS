//! Vector activation service (task-book §4 services, §12.9-10).
//!
//! Re-syncs the `active` flag in vector point payloads to match the
//! authoritative DB state after a publish / rollback. The DB is authoritative:
//! `RetrievalService` re-validates every vector hit against
//! `knowledge_documents.status`, so a vector payload that is briefly stale can
//! never surface superseded content. This makes a vector failure recoverable
//! (retry) rather than a correctness hazard, and prevents the unobservable
//! "DB active / vector inactive" divergence (§12.10).

use std::sync::Arc;

use crate::domain::rag::RAGRepoT;
use crate::domain::vector_store::{VectorDistance, VectorPoint, VectorStoreT};
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repo::{
    IngestionRunRepoT, KnowledgePublishRecord, KnowledgeVectorManifest, PublishRecordRepoT,
    VectorManifestRepoT,
};
use crate::domain::web_ingestion::status::publish_status;

/// Re-upsert all vector points of a publish record with payload lifecycle
/// fields derived from the current DB publish/run state. Requires the
/// embeddings still exist in the DB (they do — embeddings are never deleted on
/// supersede, only deactivated).
#[allow(clippy::too_many_arguments)]
pub async fn sync_active(
    vector_store: &Option<Arc<dyn VectorStoreT>>,
    vector_manifest_repo: &Arc<dyn VectorManifestRepoT>,
    publish_record_repo: &Arc<dyn PublishRecordRepoT>,
    run_repo: &Arc<dyn IngestionRunRepoT>,
    rag_repo: &Arc<dyn RAGRepoT>,
    publish_record_id: u64,
    dimension: usize,
) -> Result<(), WebIngestionError> {
    let Some(vs) = vector_store.as_ref() else {
        tracing::warn!(
            publish_record_id,
            "vector store disabled — skipping active sync (DB status is authoritative)"
        );
        return Ok(());
    };

    let manifests = vector_manifest_repo
        .list_by_publish_record(publish_record_id)
        .await?;
    if manifests.is_empty() {
        return Ok(());
    }

    let record = publish_record_repo
        .find_by_id(publish_record_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "knowledge_publish_record".into(),
            id: publish_record_id,
        })?;
    let run =
        run_repo
            .find_by_id(record.run_id)
            .await?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_ingestion_run".into(),
                id: record.run_id,
            })?;

    // All points in one collection (a publish record uses a single collection).
    let index_name = manifests[0].location.index_name.clone();
    vs.ensure_collection(&index_name, dimension, VectorDistance::Cosine)
        .await
        .map_err(|e| WebIngestionError::Internal(format!("ensure_collection: {e}")))?;

    let mut points = Vec::with_capacity(manifests.len());
    for m in &manifests {
        let vector = load_vector(rag_repo, m, dimension).await?;
        points.push(VectorPoint {
            id: m.location.point_id.clone(),
            vector,
            payload: build_payload(m, &record, run.source_url_id),
        });
    }

    vs.upsert_points(&index_name, points)
        .await
        .map_err(|e| WebIngestionError::Internal(format!("vector active re-sync: {e}")))?;
    Ok(())
}

async fn load_vector(
    rag_repo: &Arc<dyn RAGRepoT>,
    m: &KnowledgeVectorManifest,
    dimension: usize,
) -> Result<Vec<f32>, WebIngestionError> {
    let emb = rag_repo
        .find_embedding_by_chunk(m.chunk_id)
        .await
        .map_err(|e| WebIngestionError::Internal(format!("find embedding: {e}")))?
        .ok_or_else(|| {
            WebIngestionError::Internal(format!("embedding missing for chunk {}", m.chunk_id))
        })?;
    let vector: Vec<f32> = serde_json::from_value(emb.embedding_json)
        .map_err(|e| WebIngestionError::Internal(format!("decode embedding: {e}")))?;
    if vector.len() != dimension {
        return Err(WebIngestionError::EmbeddingDimensionMismatch {
            expected: dimension,
            actual: vector.len(),
        });
    }
    Ok(vector)
}

fn build_payload(
    m: &KnowledgeVectorManifest,
    record: &KnowledgePublishRecord,
    source_url_id: Option<u64>,
) -> serde_json::Value {
    let active = record.active && record.publish_status == publish_status::PUBLISHED;

    serde_json::json!({
        "source": "web_ingestion",
        "run_id": record.run_id,
        "source_id": record.source_id,
        "source_url_id": source_url_id,
        "page_id": record.page_id,
        "document_id": record.document_id,
        "version_key": record.version_key,
        "content_hash": record.content_hash,
        "active": active,
        "status": record.publish_status,
        "chunk_id": m.chunk_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::web_ingestion::repo::KnowledgePublishRecord;
    use chrono::Utc;

    fn manifest() -> KnowledgeVectorManifest {
        let now = Utc::now();
        KnowledgeVectorManifest {
            id: 10,
            publish_record_id: 20,
            run_id: 30,
            document_id: 40,
            chunk_id: 50,
            chunk_hash: "chunk-hash".into(),
            location: crate::domain::web_ingestion::repo::VectorLocation {
                index_name: "web_chunks".into(),
                point_id: "point-id".into(),
            },
            embedding_provider: "ollama".into(),
            embedding_model: "embedding-model".into(),
            embedding_dimension: 2560,
            active: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn publish_record(status: &str, active: bool) -> KnowledgePublishRecord {
        let now = Utc::now();
        KnowledgePublishRecord {
            id: 20,
            source_id: 3,
            page_id: 4,
            run_id: 30,
            document_id: 40,
            version_key: "version-key".into(),
            content_hash: "content-hash".into(),
            publish_status: status.into(),
            active,
            active_page_key: None,
            activated_at: None,
            superseded_at: None,
            superseded_by_record_id: None,
            rolled_back_from_record_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn payload_reflects_publish_record_status_not_requested_active_label() {
        let payload = build_payload(&manifest(), &publish_record("staged", false), Some(70));

        assert_eq!(payload["active"], false);
        assert_eq!(payload["status"], "staged");
        assert_eq!(payload["version_key"], "version-key");
        assert_eq!(payload["content_hash"], "content-hash");
        assert_eq!(payload["source_id"], 3);
        assert_eq!(payload["source_url_id"], 70);
        assert_eq!(payload["page_id"], 4);
        assert_eq!(payload["run_id"], 30);
        assert_eq!(payload["document_id"], 40);
        assert_eq!(payload["chunk_id"], 50);
    }
}
