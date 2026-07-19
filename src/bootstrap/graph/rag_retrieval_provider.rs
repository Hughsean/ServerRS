use std::sync::Arc;

use crate::app::rag::retrieval_service::RetrievalService;

use super::BootstrapContext;

pub(crate) fn build_rag_retrieval_service(ctx: &BootstrapContext<'_>) -> Arc<RetrievalService> {
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
            ctx.config.vector_store.rag_index_name.clone(),
        );
    }
    if ctx.config.web_ingestion.enabled {
        retrieval =
            retrieval.with_web_collection(ctx.config.web_ingestion.vector_index_name.clone());
    }

    Arc::new(retrieval)
}
