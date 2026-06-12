//! Qdrant activation service (task-book §4 services, §12.9-10).
//!
//! Re-syncs the `active` flag in Qdrant point payloads to match the
//! authoritative DB state after a publish / rollback. The DB is authoritative:
//! `RetrievalService` re-validates every Qdrant hit against
//! `knowledge_documents.status`, so a Qdrant payload that is briefly stale can
//! never surface superseded content. This makes a Qdrant failure recoverable
//! (retry) rather than a correctness hazard, and prevents the unobservable
//! "DB active / Qdrant inactive" divergence (§12.10).

use std::sync::Arc;

use crate::domain::rag::RAGRepository;
use crate::domain::vector_store::{VectorDistance, VectorPoint, VectorStore};
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::{KnowledgeVectorManifest, VectorManifestRepository};

/// Re-upsert all Qdrant points of a publish record with `active=<active>` in
/// their payload. Requires the embeddings still exist in the DB (they do —
/// embeddings are never deleted on supersede, only deactivated).
#[allow(clippy::too_many_arguments)]
pub async fn sync_active(
    vector_store: &Option<Arc<dyn VectorStore>>,
    vector_manifest_repo: &Arc<dyn VectorManifestRepository>,
    rag_repo: &Arc<dyn RAGRepository>,
    publish_record_id: u64,
    dimension: usize,
    active: bool,
) -> Result<(), WebIngestionError> {
    let Some(vs) = vector_store.as_ref() else {
        tracing::warn!(
            publish_record_id,
            "qdrant disabled — skipping active sync (DB status is authoritative)"
        );
        return Ok(());
    };

    let manifests = vector_manifest_repo
        .list_by_publish_record(publish_record_id)
        .await?;
    if manifests.is_empty() {
        return Ok(());
    }

    // All points in one collection (a publish record uses a single collection).
    let collection = manifests[0].qdrant_collection.clone();
    vs.ensure_collection(&collection, dimension, VectorDistance::Cosine)
        .await
        .map_err(|e| WebIngestionError::Internal(format!("ensure_collection: {e}")))?;

    let mut points = Vec::with_capacity(manifests.len());
    for m in &manifests {
        let vector = load_vector(rag_repo, m, dimension).await?;
        points.push(VectorPoint {
            id: m.qdrant_point_id.clone(),
            vector,
            payload: build_payload(m, active),
        });
    }

    vs.upsert_points(&collection, points)
        .await
        .map_err(|e| WebIngestionError::Internal(format!("qdrant active re-sync: {e}")))?;
    Ok(())
}

async fn load_vector(
    rag_repo: &Arc<dyn RAGRepository>,
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

fn build_payload(m: &KnowledgeVectorManifest, active: bool) -> serde_json::Value {
    serde_json::json!({
        "source": "web_ingestion",
        "run_id": m.run_id,
        "document_id": m.document_id,
        "version_key": null,
        "active": active,
        "status": if active { "published" } else { "superseded" },
        "chunk_id": m.chunk_id,
    })
}
