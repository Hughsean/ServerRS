use std::sync::Arc;

use crate::app::agent::agent_runtime::AgentTool;
use crate::app::agent::tool_registry::{AgentToolDeps, build_default_agent_tools};
use crate::app::memory::memory_service::MemoryService;
use crate::app::rag::retrieval_service::RetrievalService;

use super::BootstrapContext;

pub(crate) fn build_agent_tools(
    ctx: &BootstrapContext<'_>,
    retrieval: Arc<RetrievalService>,
    memory: Arc<MemoryService>,
) -> Result<Vec<Arc<dyn AgentTool>>, std::io::Error> {
    let tool_deps = AgentToolDeps {
        retrieval,
        memory,
        diary_repo: Arc::clone(&ctx.repos.diary_repo),
        depression_repo: Arc::clone(&ctx.repos.depression_repo),
        music_repo: Arc::clone(&ctx.repos.music_repo),
        community_repo: Arc::clone(&ctx.repos.community_repo),
        plugins: ctx.config.plugins.clone(),
    };

    build_default_agent_tools(&tool_deps, ctx.config.agent.enabled)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error.to_string()))
}
