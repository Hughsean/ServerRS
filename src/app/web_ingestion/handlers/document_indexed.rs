//! `DocumentIndexed` handler (task-book §10).
//!
//! Upserts the staged chunks' vectors into the web-ingestion Qdrant collection
//! with payload `active=false` / `status=staged` (so retrieval cannot surface
//! them), records the vector manifest, and emits `KnowledgeStaged`. Qdrant point
//! ids are deterministic (sha256(collection|chunk_hash|embedding_model)) so
//! upserts are idempotent. If Qdrant is disabled, indexing is skipped but the
//! manifest still records intent. Idempotent + resumable per §5.8.

use crate::app::web_ingestion::event_types::event as ev;
use crate::app::web_ingestion::handlers::document_chunked::WEB_SOURCE_TYPE;
use crate::app::web_ingestion::hash;
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::services::terminal_events;
use crate::app::web_ingestion::state_machine_adapter as sm;
use crate::domain::vector_store::{VectorPoint, VectorStoreT};
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::{
    DomainEvent, KnowledgeChunkManifest, NewAuditLog, NewVectorManifest,
};
use crate::domain::web_ingestion::status::{is_terminal_run_status, run_stage, run_status};
use std::sync::Arc;

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
        run_stage::EMBEDDED | run_stage::INDEXING => {} // entry / mid (resume)
        run_stage::INDEXED | run_stage::STAGING | run_stage::PUBLISHING => {
            tracing::info!(run_id, stage = %run.stage, "DocumentIndexed: already past — idempotent");
            return Ok(());
        }
        other => {
            return Err(WebIngestionError::Internal(format!(
                "DocumentIndexed: unexpected stage '{other}' for run {run_id}"
            )));
        }
    }

    let publish_record = ctx
        .publish_repo
        .find_by_run_id(run_id)
        .await?
        .ok_or_else(|| {
            WebIngestionError::Internal("DocumentIndexed: publish record missing".into())
        })?;
    let document = ctx
        .rag_repo
        .find_document_by_source(WEB_SOURCE_TYPE, Some(run_id))
        .await
        .map_err(|e| WebIngestionError::Internal(format!("find staged document: {e}")))?
        .ok_or_else(|| {
            WebIngestionError::Internal("DocumentIndexed: staged document missing".into())
        })?;
    let manifests = ctx
        .chunk_manifest_repo
        .list_by_publish_record(publish_record.id)
        .await?;
    if manifests.is_empty() {
        return Err(WebIngestionError::Internal(
            "DocumentIndexed: chunk manifest empty".into(),
        ));
    }

    // embedded → indexing (only when entering at embedded)
    if run.stage == run_stage::EMBEDDED
        && !sm::transition(
            &ctx.run_repo,
            run_id,
            run_status::RUNNING,
            run_stage::EMBEDDED,
            run_status::RUNNING,
            run_stage::INDEXING,
            None,
        )
        .await?
        .applied()
    {
        tracing::info!(
            run_id,
            "DocumentIndexed: not at embedded — concurrent worker"
        );
        return Ok(());
    }

    let collection = ctx.config.qdrant_collection.clone();
    let embedding_model = ctx.embedding_model().to_string();
    let embedding_provider = ctx.embedding_provider_name().to_string();
    let dimension = ctx.embedding_dimension();
    tracing::trace!(
        run_id,
        source_id = run.source_id,
        source_url_id = ?run.source_url_id,
        page_id = run.page_id,
        document_id = document.document_id,
        publish_record_id = publish_record.id,
        chunk_manifest_count = manifests.len(),
        collection = %collection,
        embedding_provider = %embedding_provider,
        embedding_model = %embedding_model,
        dimension,
        "DocumentIndexed: preparing vector manifest"
    );

    // ── Build vector manifest rows + upsert to Qdrant (active=false) ───────
    let mut new_manifests = Vec::with_capacity(manifests.len());
    for m in &manifests {
        let point_id = hash::qdrant_point_id(&collection, &m.chunk_hash, &embedding_model);
        new_manifests.push(NewVectorManifest {
            publish_record_id: publish_record.id,
            run_id,
            document_id: document.document_id,
            chunk_id: m.chunk_id,
            chunk_hash: m.chunk_hash.clone(),
            qdrant_collection: collection.clone(),
            qdrant_point_id: point_id,
            embedding_provider: embedding_provider.clone(),
            embedding_model: embedding_model.clone(),
            embedding_dimension: dimension as u32,
        });
    }
    tracing::trace!(
        run_id,
        document_id = document.document_id,
        publish_record_id = publish_record.id,
        point_count = new_manifests.len(),
        collection = %collection,
        "DocumentIndexed: vector manifest built"
    );

    if let Some(vs) = ctx.vector_store.as_ref() {
        upsert_points(
            ctx,
            vs,
            &collection,
            dimension,
            run_id,
            &run,
            document.document_id,
            &manifests,
            &new_manifests,
        )
        .await?;
    } else {
        tracing::warn!(
            run_id,
            "DocumentIndexed: vector_store disabled — recording manifest only"
        );
    }

    // Persist the vector manifest (idempotent via UNIQUE(chunk_id, model)).
    if ctx
        .vector_manifest_repo
        .insert_batch(&new_manifests)
        .await
        .is_err()
    {
        let existing = ctx
            .vector_manifest_repo
            .list_by_publish_record(publish_record.id)
            .await?;
        if existing.len() < new_manifests.len() {
            return Err(WebIngestionError::Internal(format!(
                "DocumentIndexed: vector manifest incomplete ({}/{})",
                existing.len(),
                new_manifests.len()
            )));
        }
    }
    tracing::trace!(
        run_id,
        document_id = document.document_id,
        publish_record_id = publish_record.id,
        vector_manifest_count = new_manifests.len(),
        "DocumentIndexed: vector manifest persisted"
    );

    ctx.audit_repo
        .insert(NewAuditLog {
            source_id: Some(run.source_id),
            source_url_id: run.source_url_id,
            page_id: Some(run.page_id),
            run_id: Some(run_id),
            publish_record_id: Some(publish_record.id),
            action: "document_indexed".into(),
            status: "success".into(),
            message: format!(
                "indexed {} points to '{}' (active=false)",
                new_manifests.len(),
                collection
            ),
            metadata: Some(serde_json::json!({
                "qdrant_collection": collection,
                "point_count": new_manifests.len(),
                "active": false,
            })),
        })
        .await?;

    // indexing → indexed
    let _ = sm::transition(
        &ctx.run_repo,
        run_id,
        run_status::RUNNING,
        run_stage::INDEXING,
        run_status::RUNNING,
        run_stage::INDEXED,
        None,
    )
    .await?;

    tracing::trace!(
        run_id,
        document_id = document.document_id,
        publish_record_id = publish_record.id,
        "DocumentIndexed: indexing complete; emitting KnowledgeStaged"
    );
    terminal_events::emit_next(
        &ctx.outbox_repo,
        ev::KNOWLEDGE_STAGED,
        run_id,
        &run.version_key,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn upsert_points(
    ctx: &PipelineContext,
    vs: &Arc<dyn VectorStoreT>,
    collection: &str,
    dimension: usize,
    run_id: u64,
    run: &crate::domain::web_ingestion::repository::KnowledgeIngestionRun,
    document_id: u64,
    chunk_manifests: &[KnowledgeChunkManifest],
    vector_manifests: &[NewVectorManifest],
) -> Result<(), WebIngestionError> {
    use crate::domain::vector_store::VectorDistance;

    // Ensure the web-ingestion collection exists (separate from legacy RAG).
    vs.ensure_collection(collection, dimension, VectorDistance::Cosine)
        .await
        .map_err(|e| WebIngestionError::Internal(format!("ensure_collection: {e}")))?;
    tracing::trace!(
        run_id,
        collection,
        dimension,
        point_count = vector_manifests.len(),
        "DocumentIndexed: qdrant collection ready"
    );

    let mut points = Vec::with_capacity(vector_manifests.len());
    for (cm, vm) in chunk_manifests.iter().zip(vector_manifests.iter()) {
        // Re-fetch the persisted embedding for this chunk.
        let emb = ctx
            .rag_repo
            .find_embedding_by_chunk(cm.chunk_id)
            .await
            .map_err(|e| WebIngestionError::Internal(format!("find embedding: {e}")))?
            .ok_or_else(|| {
                WebIngestionError::Internal(format!(
                    "DocumentIndexed: embedding missing for chunk {}",
                    cm.chunk_id
                ))
            })?;
        let vector: Vec<f32> = serde_json::from_value(emb.embedding_json)
            .map_err(|e| WebIngestionError::Internal(format!("decode embedding: {e}")))?;
        if vector.len() != dimension {
            return Err(WebIngestionError::EmbeddingDimensionMismatch {
                expected: dimension,
                actual: vector.len(),
            });
        }
        let payload = serde_json::json!({
            "source": "web_ingestion",
            "run_id": run_id,
            "page_id": run.page_id,
            "source_id": run.source_id,
            "source_url_id": run.source_url_id,
            "version_key": run.version_key,
            "content_hash": run.content_hash,
            "active": false,
            "status": "staged",
            "chunk_id": cm.chunk_id,
            "document_id": document_id,
        });
        points.push(VectorPoint {
            id: vm.qdrant_point_id.clone(),
            vector,
            payload,
        });
    }

    let point_count = points.len();
    tracing::trace!(
        run_id,
        collection,
        point_count,
        dimension,
        "DocumentIndexed: qdrant upsert started"
    );
    vs.upsert_points(collection, points)
        .await
        .map_err(|e| WebIngestionError::Internal(format!("qdrant upsert: {e}")))?;
    tracing::trace!(
        run_id,
        collection,
        point_count,
        "DocumentIndexed: qdrant upsert completed"
    );
    Ok(())
}
