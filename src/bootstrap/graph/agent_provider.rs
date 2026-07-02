use std::sync::Arc;

use crate::app::agent::agent_context::AgentContextBuilder;
use crate::app::agent::agent_runtime::{AgentRuntime, AgentRuntimeSettings};
use crate::app::agent::tool_registry::{AgentToolDeps, build_default_agent_tools};
use crate::app::context_routing::ContextRoutingService;
use crate::app::fresh_context::retrieval::FreshRetrievalService;
use crate::app::memory::memory_service::MemoryService;
use crate::app::rag::retrieval_service::RetrievalService;
use crate::app::summary::summary_service::SummaryService;
use crate::domain::fresh_context::FreshContextRepoT;
use crate::domain::semantic_classification::SemanticClassifierT;
use crate::infra::db::imp::fresh_context_repo::FreshContextRepo;
use crate::infra::semantic_classification::EmbeddingSemanticClassifier;

use super::BootstrapContext;

pub struct AgentServices {
    pub runtime: Arc<AgentRuntime>,
}

pub async fn build_agent_services(
    ctx: &BootstrapContext<'_>,
    retrieval: Arc<RetrievalService>,
    memory: Arc<MemoryService>,
    summary: Arc<SummaryService>,
) -> Result<AgentServices, std::io::Error> {
    let context_builder = Arc::new(
        AgentContextBuilder::new(
            Arc::clone(&memory),
            Arc::clone(&retrieval),
            Arc::clone(&summary),
            build_fresh_retrieval_service(ctx),
            Arc::clone(&ctx.repos.conv_repo),
            Arc::clone(&ctx.repos.profile_repo),
        )
        .with_context_routing_service(build_context_routing_service(ctx).await?),
    );

    let tool_deps = AgentToolDeps {
        retrieval,
        memory: Arc::clone(&memory),
        diary_repo: Arc::clone(&ctx.repos.diary_repo),
        depression_repo: Arc::clone(&ctx.repos.depression_repo),
        music_repo: Arc::clone(&ctx.repos.music_repo),
        community_repo: Arc::clone(&ctx.repos.community_repo),
        plugins: ctx.config.plugins.clone(),
    };

    let tools = build_default_agent_tools(&tool_deps, ctx.config.agent.enabled)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error.to_string()))?;

    let runtime = Arc::new(AgentRuntime::new(
        Arc::clone(&ctx.infra.ollama_provider),
        memory,
        Arc::clone(&ctx.repos.agent_event_repo),
        Arc::clone(&ctx.repos.conv_repo),
        Arc::clone(&ctx.repos.profile_repo),
        Arc::clone(&ctx.repos.context_version_repo),
        context_builder,
        tools,
        agent_runtime_settings(ctx),
    ));

    Ok(AgentServices { runtime })
}

fn build_fresh_retrieval_service(ctx: &BootstrapContext<'_>) -> Option<Arc<FreshRetrievalService>> {
    if !ctx.config.fresh_context.enabled {
        return None;
    }

    ctx.vector.vector_store.as_ref().map(|vector_store| {
        let fresh_repo: Arc<dyn FreshContextRepoT> =
            Arc::new(FreshContextRepo::new(ctx.infra.db.clone()));
        Arc::new(FreshRetrievalService::new(
            fresh_repo,
            Arc::clone(vector_store),
            Arc::clone(&ctx.vector.embedding_provider),
            ctx.config.fresh_context.clone(),
        ))
    })
}

async fn build_context_routing_service(
    ctx: &BootstrapContext<'_>,
) -> Result<Option<Arc<ContextRoutingService>>, std::io::Error> {
    if !ctx.config.context_routing.enabled {
        return Ok(None);
    }

    let classifier: Arc<dyn SemanticClassifierT> = Arc::new(
        EmbeddingSemanticClassifier::from_config(
            &ctx.config.semantic_classification,
            Arc::clone(&ctx.vector.embedding_provider),
        )
        .await
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("上下文路由分类器初始化失败: {error}"),
            )
        })?,
    );

    Ok(Some(Arc::new(ContextRoutingService::new(
        classifier,
        ctx.config.context_routing.clone(),
    ))))
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
