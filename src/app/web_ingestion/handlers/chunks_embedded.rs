//! `ChunksEmbedded` handler (task-book §9).
//!
//! Loads the staged chunks for the run's document, embeds them in BATCHES via
//! the embedding provider (separate from the distill chat LLM — §9.1), validates
//! the returned dimension, persists embeddings (knowledge_embeddings), and emits
//! `DocumentIndexed`. Idempotent + resumable per §5.8.

use crate::app::web_ingestion::event_types::event as ev;
use crate::app::web_ingestion::handlers::document_chunked::WEB_SOURCE_TYPE;
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::services::terminal_events;
use crate::app::web_ingestion::state_machine_adapter as sm;
use crate::domain::rag::{KnowledgeChunk, NewEmbedding};
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
        run_stage::CHUNKED | run_stage::EMBEDDING => {} // entry / mid (resume)
        run_stage::EMBEDDED
        | run_stage::INDEXING
        | run_stage::INDEXED
        | run_stage::STAGING
        | run_stage::PUBLISHING => {
            tracing::info!(run_id, stage = %run.stage, "ChunksEmbedded: already past — idempotent");
            return Ok(());
        }
        other => {
            return Err(WebIngestionError::Internal(format!(
                "ChunksEmbedded: unexpected stage '{other}' for run {run_id}"
            )));
        }
    }

    // ── Locate the staged document + its chunks ────────────────────────────
    let document = ctx
        .rag_repo
        .find_document_by_source(WEB_SOURCE_TYPE, Some(run_id))
        .await
        .map_err(|e| WebIngestionError::Internal(format!("find staged document: {e}")))?
        .ok_or_else(|| {
            WebIngestionError::Internal("ChunksEmbedded: staged document missing".into())
        })?;
    let chunks = ctx
        .rag_repo
        .find_chunks_by_document(document.document_id)
        .await
        .map_err(|e| WebIngestionError::Internal(format!("find chunks: {e}")))?;
    if chunks.is_empty() {
        return Err(WebIngestionError::Internal(
            "ChunksEmbedded: no chunks for staged document".into(),
        ));
    }

    // chunked → embedding (only when entering at chunked)
    if run.stage == run_stage::CHUNKED
        && !sm::transition(
            &ctx.run_repo,
            run_id,
            run_status::RUNNING,
            run_stage::CHUNKED,
            run_status::RUNNING,
            run_stage::EMBEDDING,
            None,
        )
        .await?
        .applied()
    {
        tracing::info!(run_id, "ChunksEmbedded: not at chunked — concurrent worker");
        return Ok(());
    }

    // ── Batch embedding (§9.2: never one-request-per-chunk) ────────────────
    let expected_dim = ctx.embedding_dimension();
    let batch_size = ctx.config.embedding_batch_size.max(1);
    let embedded = embed_missing(ctx, &chunks, batch_size, expected_dim).await?;

    ctx.run_repo
        .update_embedding_info(
            run_id,
            ctx.embedding_provider_name(),
            ctx.embedding_model(),
            expected_dim as u32,
        )
        .await?;

    ctx.audit_repo
        .insert(NewAuditLog {
            source_id: Some(run.source_id),
            source_url_id: run.source_url_id,
            page_id: Some(run.page_id),
            run_id: Some(run_id),
            publish_record_id: None,
            action: "chunks_embedded".into(),
            status: "success".into(),
            message: format!("embedded {embedded} chunks (batch_size={batch_size})"),
            metadata: Some(serde_json::json!({
                "embedding_provider": ctx.embedding_provider_name(),
                "embedding_model": ctx.embedding_model(),
                "embedding_dimension": expected_dim,
                "chunk_count": chunks.len(),
            })),
        })
        .await?;

    // embedding → embedded
    let _ = sm::transition(
        &ctx.run_repo,
        run_id,
        run_status::RUNNING,
        run_stage::EMBEDDING,
        run_status::RUNNING,
        run_stage::EMBEDDED,
        None,
    )
    .await?;

    terminal_events::emit_next(
        &ctx.outbox_repo,
        ev::DOCUMENT_INDEXED,
        run_id,
        &run.version_key,
    )
    .await
}

/// Embed only the chunks that do not yet have a persisted embedding (resume),
/// in batches. Validates dimension and count per batch. Returns the number of
/// embeddings written this call.
async fn embed_missing(
    ctx: &PipelineContext,
    chunks: &[KnowledgeChunk],
    batch_size: usize,
    expected_dim: usize,
) -> Result<usize, WebIngestionError> {
    // Determine which chunks still need embeddings.
    let mut pending: Vec<&KnowledgeChunk> = Vec::new();
    for chunk in chunks {
        let has = ctx
            .rag_repo
            .find_embedding_by_chunk(chunk.chunk_id)
            .await
            .map_err(|e| WebIngestionError::Internal(format!("find embedding: {e}")))?
            .is_some();
        if !has {
            pending.push(chunk);
        }
    }
    if pending.is_empty() {
        return Ok(0);
    }

    let mut written = 0usize;
    for batch in pending.chunks(batch_size) {
        let texts: Vec<String> = batch.iter().map(|c| c.content.clone()).collect();
        let vectors = ctx
            .embedding_provider
            .embed(&texts)
            .await
            .map_err(|e| WebIngestionError::Internal(format!("embedding provider: {e}")))?;

        if vectors.len() != batch.len() {
            return Err(WebIngestionError::EmbeddingCountMismatch {
                expected: batch.len(),
                actual: vectors.len(),
            });
        }

        for (chunk, vector) in batch.iter().zip(vectors.into_iter()) {
            if vector.len() != expected_dim {
                return Err(WebIngestionError::EmbeddingDimensionMismatch {
                    expected: expected_dim,
                    actual: vector.len(),
                });
            }
            let embedding_json = serde_json::to_value(&vector).map_err(|e| {
                WebIngestionError::Internal(format!("serialize embedding vector: {e}"))
            })?;
            ctx.rag_repo
                .save_embedding(NewEmbedding {
                    chunk_id: chunk.chunk_id,
                    provider: ctx.embedding_provider_name().to_string(),
                    model: ctx.embedding_model().to_string(),
                    dimension: expected_dim as u32,
                    embedding_json,
                })
                .await
                .map_err(|e| WebIngestionError::Internal(format!("save embedding: {e}")))?;
            written += 1;
        }
    }
    Ok(written)
}
