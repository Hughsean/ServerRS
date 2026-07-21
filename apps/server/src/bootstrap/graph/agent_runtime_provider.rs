use std::sync::Arc;

use crate::app::agent::agent_context::AgentContextBuilder;
use crate::app::agent::agent_runtime::{AgentRuntime, AgentRuntimeSettings, AgentTool};
use crate::app::agent::chat_effect::ConversationTurnWriter;
use crate::app::agent::chat_graph::{ChatAgentGraph, ChatAgentGraphDeps};
use crate::app::agent::memory_extraction::AsyncMemoryExtractionScheduler;
use crate::app::agent::nodes::DefaultChatContextProvider;
use crate::app::memory::memory_service::MemoryService;

use super::BootstrapContext;

pub(crate) fn build_agent_runtime(
    ctx: &BootstrapContext<'_>,
    memory: Arc<MemoryService>,
    context_builder: Arc<AgentContextBuilder>,
    tools: Vec<Arc<dyn AgentTool>>,
) -> Arc<AgentRuntime> {
    let settings = agent_runtime_settings(ctx);
    let max_context_messages = settings.max_context_messages;
    let context_provider = Arc::new(DefaultChatContextProvider::new(
        Arc::clone(&ctx.repos.context_version_repo),
        Arc::clone(&ctx.repos.profile_repo),
        context_builder,
    ));
    let turn_writer = Arc::new(ConversationTurnWriter::new(Arc::clone(
        &ctx.repos.conv_repo,
    )));
    let memory_extraction_scheduler = Arc::new(AsyncMemoryExtractionScheduler::new(memory));
    let chat_graph = ChatAgentGraph::new(ChatAgentGraphDeps {
        llm: Arc::clone(&ctx.infra.ollama_provider),
        event_repo: Arc::clone(&ctx.repos.agent_event_repo),
        context_provider,
        turn_writer,
        memory_extraction_scheduler,
        tools,
        settings,
    })
    .expect("静态 HTTP Chat Agent 图必须能够编译");

    Arc::new(AgentRuntime::from_graph(chat_graph, max_context_messages))
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
