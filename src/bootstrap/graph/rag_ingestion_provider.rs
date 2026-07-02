use std::sync::Arc;

use crate::app::rag::chunking::ChunkingService;
use crate::app::rag::ingestion_service::IngestionService;

use super::BootstrapContext;

pub(crate) fn build_rag_ingestion_service(ctx: &BootstrapContext<'_>) -> Arc<IngestionService> {
    let chunking = ChunkingService::new();
    let mut ingestion = IngestionService::new(
        Arc::clone(&ctx.repos.rag_repo),
        chunking,
        Some(Arc::clone(&ctx.vector.embedding_provider)),
    )
    .with_chunking_config(ctx.config.rag.chunk_size, ctx.config.rag.chunk_overlap);

    if let Some(vector_index) = &ctx.vector.vector_index {
        ingestion = ingestion.with_vector_index(Arc::clone(vector_index));
    }

    Arc::new(ingestion)
}
