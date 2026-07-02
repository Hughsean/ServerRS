use std::sync::Arc;

use crate::app::agent::agent_context::AgentContextBuilder;
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

pub(crate) async fn build_agent_context_builder(
    ctx: &BootstrapContext<'_>,
    retrieval: Arc<RetrievalService>,
    memory: Arc<MemoryService>,
    summary: Arc<SummaryService>,
) -> Result<Arc<AgentContextBuilder>, std::io::Error> {
    Ok(Arc::new(
        AgentContextBuilder::new(
            memory,
            retrieval,
            summary,
            build_fresh_retrieval_service(ctx),
            Arc::clone(&ctx.repos.conv_repo),
            Arc::clone(&ctx.repos.profile_repo),
        )
        .with_context_routing_service(build_context_routing_service(ctx).await?),
    ))
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
