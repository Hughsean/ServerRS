use std::sync::Arc;

use crate::app::memory::memory_service::MemoryService;

use super::{
    BootstrapContext, memory_extractor_provider::build_memory_extractor,
    memory_service_provider::build_configured_memory_service,
};

pub struct MemoryServices {
    pub memory: Arc<MemoryService>,
}

pub fn build_memory_services(ctx: &BootstrapContext<'_>) -> MemoryServices {
    let memory_extractor = build_memory_extractor(ctx);
    let memory = build_configured_memory_service(ctx, memory_extractor);

    MemoryServices { memory }
}
