//! `DocumentChunked` handler (task-book §8).
//!
//! Generates chunks from the distilled document via the industrial chunker,
//! creates a STAGED publish record (active=0), writes a staged
//! `knowledge_documents` row (status=0 → invisible to retrieval) plus its
//! chunks, records the chunk manifest, and emits `ChunksEmbedded`.
//!
//! Version isolation: one `knowledge_documents` row per run
//! (source_type="web_ingestion", source_id=run_id) so different versions never
//! collide on UNIQUE(source_type, source_id) or UNIQUE(document_id, chunk_index).
//! Idempotent + resumable per §5.8 (deterministic chunk_hash + manifest dedup).

use crate::app::web_ingestion::event_types::event as ev;
use crate::app::web_ingestion::industrial_chunker::{self, ChunkerConfig, SectionInput};
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::services::terminal_events;
use crate::app::web_ingestion::state_machine_adapter as sm;
use crate::domain::rag::{NewChunk, NewDocument};
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::{
    DomainEvent, KnowledgeIngestionRun, NewAuditLog, NewChunkManifest, NewPublishRecord,
};
use crate::domain::web_ingestion::status::{is_terminal_run_status, run_stage, run_status};

/// web_ingestion documents use this source_type so they are partitioned from
/// legacy RAG documents (source_type != "web_ingestion").
pub const WEB_SOURCE_TYPE: &str = "web_ingestion";
/// Staged knowledge_documents/chunks are written with status=0 → excluded by
/// RetrievalService (which requires status==1). Publish flips this to 1.
pub const STATUS_STAGED: i8 = 0;

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
        run_stage::CHUNKING => {} // entry / mid (resume — manifest dedups)
        run_stage::CHUNKED
        | run_stage::EMBEDDING
        | run_stage::EMBEDDED
        | run_stage::INDEXING
        | run_stage::INDEXED
        | run_stage::STAGING
        | run_stage::PUBLISHING => {
            tracing::info!(run_id, stage = %run.stage, "DocumentChunked: already past — idempotent");
            return Ok(());
        }
        other => {
            return Err(WebIngestionError::Internal(format!(
                "DocumentChunked: unexpected stage '{other}' for run {run_id}"
            )));
        }
    }

    let distilled = run.distilled_json.as_ref().ok_or_else(|| {
        WebIngestionError::Internal("DocumentChunked: distilled_json missing".into())
    })?;

    // ── Build chunker inputs from the distilled document ───────────────────
    let title = distilled["title"].as_str().unwrap_or("").to_string();
    let summary = distilled["summary"].as_str().unwrap_or("").to_string();
    let page = ctx
        .page_repo
        .find_by_id(run.page_id)
        .await?
        .ok_or_else(|| WebIngestionError::NotFound {
            entity: "web_page".into(),
            id: run.page_id,
        })?;
    let source_url = page
        .canonical_url
        .as_deref()
        .unwrap_or(page.url.as_str())
        .to_string();
    let sections: Vec<SectionInput> = distilled["sections"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|s| SectionInput {
                    heading: s["heading"].as_str().unwrap_or("").to_string(),
                    body: s["body"].as_str().unwrap_or("").to_string(),
                    summary: s["summary"].as_str().map(String::from),
                })
                .collect()
        })
        .unwrap_or_default();

    let chunker_config = ChunkerConfig {
        target_min: ctx.config.chunk_target_min,
        target_max: ctx.config.chunk_target_max,
        overlap_min: ctx.config.chunk_overlap_min,
        overlap_max: ctx.config.chunk_overlap_max,
        chunker_version: ctx.chunker_version().to_string(),
    };

    let chunk_outputs = industrial_chunker::chunk_document(
        &title,
        &summary,
        &source_url,
        &sections,
        &run.version_key,
        &chunker_config,
    );
    tracing::debug!(
        run_id,
        source_id = run.source_id,
        source_url_id = ?run.source_url_id,
        page_id = run.page_id,
        title = %title,
        sections = sections.len(),
        chunk_count = chunk_outputs.len(),
        target_min = chunker_config.target_min,
        target_max = chunker_config.target_max,
        chunker_version = %chunker_config.chunker_version,
        "DocumentChunked: chunker completed"
    );

    if chunk_outputs.is_empty() {
        // Nothing to chunk — reject (cannot index an empty document).
        let _ = sm::transition(
            &ctx.run_repo,
            run_id,
            run_status::RUNNING,
            run_stage::CHUNKING,
            run_status::REJECTED,
            run_stage::REJECTED,
            Some("chunker produced no chunks"),
        )
        .await?;
        terminal_events::emit_rejected(&ctx.outbox_repo, run_id, &run.version_key, "no_chunks")
            .await?;
        return Ok(());
    }

    // ── Staged document + publish record (active=0) + chunks ──────────────
    // Order: document → publish record (needs document_id) → chunks → manifest.
    let document_id = ensure_staged_document(ctx, &run, &source_url).await?;
    let publish_record_id = ensure_publish_record(ctx, &run, document_id).await?;
    let saved_chunks = ensure_chunks(ctx, document_id, &chunk_outputs).await?;
    tracing::debug!(
        run_id,
        document_id,
        publish_record_id,
        saved_chunks = saved_chunks.len(),
        expected_chunks = chunk_outputs.len(),
        "DocumentChunked: staged document and chunks ensured"
    );

    // Alignment guard: the persisted chunks MUST correspond 1:1 (and in
    // chunk_index order) to the chunker output before we build the manifest.
    // A mismatch means a corrupt/partial prior write — fail so the event is
    // retried rather than cementing a truncated, misaligned manifest.
    if saved_chunks.len() != chunk_outputs.len() {
        return Err(WebIngestionError::Internal(format!(
            "DocumentChunked: persisted chunk count {} != chunker output {} for run {run_id} — \
             refusing to build a partial manifest",
            saved_chunks.len(),
            chunk_outputs.len()
        )));
    }
    for (out, chunk) in chunk_outputs.iter().zip(saved_chunks.iter()) {
        if out.chunk_index != chunk.chunk_index {
            return Err(WebIngestionError::Internal(format!(
                "DocumentChunked: chunk_index misalignment (chunker={}, persisted={}) for run {run_id}",
                out.chunk_index, chunk.chunk_index
            )));
        }
    }

    // ── Chunk manifest (idempotent via UNIQUE(version_key, chunk_hash)) ────
    let manifests: Vec<NewChunkManifest> = chunk_outputs
        .iter()
        .zip(saved_chunks.iter())
        .map(|(out, chunk)| NewChunkManifest {
            publish_record_id,
            run_id,
            document_id,
            chunk_id: chunk.chunk_id,
            version_key: run.version_key.clone(),
            chunk_hash: out.chunk_hash.clone(),
            chunk_type: out.chunk_type_str().to_string(),
            chunk_index: out.chunk_index,
        })
        .collect();

    // insert_batch is idempotent at the row level via UNIQUE keys; on a resume
    // some rows may already exist, so tolerate duplicate-key by re-querying and
    // comparing against the FULL expected count (chunk_outputs.len()).
    if ctx
        .chunk_manifest_repo
        .insert_batch(&manifests)
        .await
        .is_err()
    {
        let existing = ctx
            .chunk_manifest_repo
            .list_by_publish_record(publish_record_id)
            .await?;
        if existing.len() < chunk_outputs.len() {
            return Err(WebIngestionError::Internal(format!(
                "DocumentChunked: chunk manifest incomplete ({}/{})",
                existing.len(),
                chunk_outputs.len()
            )));
        }
    }

    ctx.audit_repo
        .insert(NewAuditLog {
            source_id: Some(run.source_id),
            source_url_id: run.source_url_id,
            page_id: Some(run.page_id),
            run_id: Some(run_id),
            publish_record_id: Some(publish_record_id),
            action: "document_chunked".into(),
            status: "success".into(),
            message: format!("chunked into {} chunks", chunk_outputs.len()),
            metadata: Some(serde_json::json!({
                "chunk_count": chunk_outputs.len(),
                "chunker_version": ctx.chunker_version(),
                "document_id": document_id,
            })),
        })
        .await?;

    // running/chunking → running/chunked
    let _ = sm::transition(
        &ctx.run_repo,
        run_id,
        run_status::RUNNING,
        run_stage::CHUNKING,
        run_status::RUNNING,
        run_stage::CHUNKED,
        None,
    )
    .await?;

    tracing::debug!(
        run_id,
        document_id,
        publish_record_id,
        chunk_count = chunk_outputs.len(),
        "DocumentChunked: manifest persisted; emitting ChunksEmbedded"
    );
    terminal_events::emit_next(
        &ctx.outbox_repo,
        ev::CHUNKS_EMBEDDED,
        run_id,
        &run.version_key,
    )
    .await
}

/// Find or create the staged publish record for this run (active=0).
async fn ensure_publish_record(
    ctx: &PipelineContext,
    run: &KnowledgeIngestionRun,
    document_id: u64,
) -> Result<u64, WebIngestionError> {
    if let Some(existing) = ctx.publish_repo.find_by_run_id(run.id).await? {
        return Ok(existing.id);
    }
    let record = ctx
        .publish_repo
        .insert(NewPublishRecord {
            source_id: run.source_id,
            page_id: run.page_id,
            run_id: run.id,
            document_id,
            version_key: run.version_key.clone(),
            content_hash: run.content_hash.clone(),
            active_page_key: None, // staged → not active
        })
        .await?;
    Ok(record.id)
}

/// Find or create the staged knowledge_documents row for this run.
/// source_type="web_ingestion", source_id=run_id → one document per version.
async fn ensure_staged_document(
    ctx: &PipelineContext,
    run: &KnowledgeIngestionRun,
    source_url: &str,
) -> Result<u64, WebIngestionError> {
    if let Some(existing) = ctx
        .rag_repo
        .find_document_by_source(WEB_SOURCE_TYPE, Some(run.id))
        .await
        .map_err(|e| WebIngestionError::Internal(format!("find staged document: {e}")))?
    {
        return Ok(existing.document_id);
    }
    let title = run
        .distilled_json
        .as_ref()
        .and_then(|d| d["title"].as_str())
        .map(String::from);
    let doc = ctx
        .rag_repo
        .save_document(NewDocument {
            source_type: WEB_SOURCE_TYPE.to_string(),
            source_id: Some(run.id),
            title,
            content_hash: run.content_hash.clone(),
            metadata: Some(serde_json::json!({
                "run_id": run.id,
                "version_key": run.version_key,
                "page_id": run.page_id,
                "web_source_id": run.source_id,
                "source_url": source_url,
            })),
            // Staged → status=0 so RetrievalService (requires status==1) cannot
            // surface it until publish flips it to 1.
            status: STATUS_STAGED,
        })
        .await
        .map_err(|e| WebIngestionError::Internal(format!("save staged document: {e}")))?;
    Ok(doc.document_id)
}

/// Find or create the chunk rows for the staged document.
async fn ensure_chunks(
    ctx: &PipelineContext,
    document_id: u64,
    chunk_outputs: &[industrial_chunker::ChunkOutput],
) -> Result<Vec<crate::domain::rag::KnowledgeChunk>, WebIngestionError> {
    let existing = ctx
        .rag_repo
        .find_chunks_by_document(document_id)
        .await
        .map_err(|e| WebIngestionError::Internal(format!("find chunks: {e}")))?;
    if existing.len() == chunk_outputs.len() {
        return Ok(existing);
    }
    if !existing.is_empty() {
        // A nonzero-but-incomplete set means a corrupt/partial prior write.
        // save_chunks inserts all rows in one insert_many, so this should not
        // happen normally — fail loudly rather than build a partial manifest.
        return Err(WebIngestionError::Internal(format!(
            "DocumentChunked: document {document_id} has {} chunks but chunker produced {} — \
             inconsistent state, refusing to proceed",
            existing.len(),
            chunk_outputs.len()
        )));
    }
    let new_chunks: Vec<NewChunk> = chunk_outputs
        .iter()
        .map(|out| NewChunk {
            document_id,
            chunk_index: out.chunk_index,
            content: out.content.clone(),
            token_count: Some(out.content.chars().count() as u32),
            metadata: Some(serde_json::json!({
                "chunk_type": out.chunk_type_str(),
                "chunk_hash": out.chunk_hash,
            })),
        })
        .collect();
    ctx.rag_repo
        .save_chunks(&new_chunks)
        .await
        .map_err(|e| WebIngestionError::Internal(format!("save chunks: {e}")))
}
