use std::sync::Arc;

use crate::app::memory::memory_extractor::MemoryExtractor;

use super::BootstrapContext;

pub(crate) fn build_memory_extractor(ctx: &BootstrapContext<'_>) -> Arc<MemoryExtractor> {
    Arc::new(MemoryExtractor::new(Arc::clone(&ctx.infra.ollama_provider)))
}
