use std::sync::Arc;

use crate::app::rag::chunking::ChunkingService;
use crate::app::rag::ingestion_service::IngestionService;
use crate::app::rag::retrieval_service::RetrievalService;

use super::BootstrapContext;

pub struct RagServices {
    pub retrieval: Arc<RetrievalService>,
    pub ingestion: Arc<IngestionService>,
}

pub fn build_rag_services(ctx: &BootstrapContext<'_>) -> RagServices {
    let mut retrieval = RetrievalService::new(
        Arc::clone(&ctx.repos.rag_repo),
        Some(Arc::clone(&ctx.vector.embedding_provider)),
    )
    .with_hybrid_weights(
        ctx.config.rag.hybrid_vector_weight,
        ctx.config.rag.hybrid_keyword_weight,
    );

    if let Some(vector_store) = &ctx.vector.vector_store {
        retrieval = retrieval.with_vector_store(
            Arc::clone(vector_store),
            ctx.config.qdrant.rag_collection.clone(),
        );
    }
    if ctx.config.web_ingestion.enabled {
        retrieval =
            retrieval.with_web_collection(ctx.config.web_ingestion.qdrant_collection.clone());
    }

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

    RagServices {
        retrieval: Arc::new(retrieval),
        ingestion: Arc::new(ingestion),
    }
}
