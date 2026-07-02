use std::sync::Arc;

use crate::app::agent::agent_runtime::AgentRuntime;
use crate::app::memory::memory_service::MemoryService;
use crate::app::rag::retrieval_service::RetrievalService;
use crate::app::summary::summary_service::SummaryService;

use super::BootstrapContext;
use super::agent_context_provider::build_agent_context_builder;
use super::agent_runtime_provider::build_agent_runtime;
use super::agent_tool_provider::build_agent_tools;

pub struct AgentServices {
    pub runtime: Arc<AgentRuntime>,
}

pub async fn build_agent_services(
    ctx: &BootstrapContext<'_>,
    retrieval: Arc<RetrievalService>,
    memory: Arc<MemoryService>,
    summary: Arc<SummaryService>,
) -> Result<AgentServices, std::io::Error> {
    let context_builder = build_agent_context_builder(
        ctx,
        Arc::clone(&retrieval),
        Arc::clone(&memory),
        Arc::clone(&summary),
    )
    .await?;
    let tools = build_agent_tools(ctx, retrieval, Arc::clone(&memory))?;
    let runtime = build_agent_runtime(ctx, memory, context_builder, tools);

    Ok(AgentServices { runtime })
}
