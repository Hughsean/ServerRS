use std::sync::Arc;

use crate::app::memory::memory_extractor::MemoryExtractor;
use crate::app::memory::memory_service::MemoryService;

use super::BootstrapContext;

pub struct MemoryServices {
    pub memory: Arc<MemoryService>,
}

pub fn build_memory_services(ctx: &BootstrapContext<'_>) -> MemoryServices {
    let memory_extractor = Arc::new(MemoryExtractor::new(Arc::clone(&ctx.infra.ollama_provider)));
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

    MemoryServices {
        memory: Arc::new(memory),
    }
}
