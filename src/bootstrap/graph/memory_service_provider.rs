use std::sync::Arc;

use crate::app::memory::memory_extractor::MemoryExtractor;
use crate::app::memory::memory_service::MemoryService;

use super::BootstrapContext;

pub(crate) fn build_configured_memory_service(
    ctx: &BootstrapContext<'_>,
    memory_extractor: Arc<MemoryExtractor>,
) -> Arc<MemoryService> {
    let mut memory = MemoryService::new(Arc::clone(&ctx.repos.memory_repo), memory_extractor)
        .with_personalization_profile_repo(Arc::clone(&ctx.repos.profile_repo))
        .with_context_version_repo(Arc::clone(&ctx.repos.context_version_repo));

    if let Some(vector_store) = &ctx.vector.vector_store {
        memory = memory.with_vector_search(
            Arc::clone(vector_store),
            Arc::clone(&ctx.vector.embedding_provider),
            ctx.config.qdrant.memory_collection.clone(),
        );
    }
    if let Some(vector_index) = &ctx.vector.vector_index {
        memory = memory.with_vector_index(Arc::clone(vector_index));
    }

    Arc::new(memory)
}
