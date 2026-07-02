use std::sync::Arc;

use crate::app::agent::agent_context::AgentContextBuilder;
use crate::app::agent::agent_runtime::{AgentRuntime, AgentRuntimeSettings, AgentTool};
use crate::app::memory::memory_service::MemoryService;

use super::BootstrapContext;

pub(crate) fn build_agent_runtime(
    ctx: &BootstrapContext<'_>,
    memory: Arc<MemoryService>,
    context_builder: Arc<AgentContextBuilder>,
    tools: Vec<Arc<dyn AgentTool>>,
) -> Arc<AgentRuntime> {
    Arc::new(AgentRuntime::new(
        Arc::clone(&ctx.infra.ollama_provider),
        memory,
        Arc::clone(&ctx.repos.agent_event_repo),
        Arc::clone(&ctx.repos.conv_repo),
        Arc::clone(&ctx.repos.profile_repo),
        Arc::clone(&ctx.repos.context_version_repo),
        context_builder,
        tools,
        agent_runtime_settings(ctx),
    ))
}

fn agent_runtime_settings(ctx: &BootstrapContext<'_>) -> AgentRuntimeSettings {
    AgentRuntimeSettings {
        agent_enabled: ctx.config.agent.enabled,
        memory_enabled: ctx.config.agent.memory_enabled,
        rag_enabled: ctx.config.agent.rag_enabled,
        summary_enabled: ctx.config.agent.summary_enabled,
        max_context_messages: ctx.config.agent.max_context_messages as usize,
        max_memory_items: ctx.config.agent.max_memory_items,
        max_rag_chunks: ctx.config.agent.max_rag_chunks as u64,
        memory_extraction_async: ctx.config.agent.memory_extraction_async,
        max_tool_depth: ctx.config.llm.max_tool_depth as usize,
        temperature: ctx.config.llm.temperature,
        top_p: ctx.config.llm.top_p,
        enable_reasoning: ctx.config.llm.enable_reasoning,
    }
}
