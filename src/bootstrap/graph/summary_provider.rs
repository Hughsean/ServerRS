use std::sync::Arc;

use crate::app::summary::summary_refresh_handler::SummaryRefreshHandler;
use crate::app::summary::summary_service::SummaryService;
use crate::domain::conversation::conversation_repository::ConversationRepoT;
use crate::domain::llm::LlmProvider;
use crate::domain::tasks::task_handler::TaskHandler;

use super::BootstrapContext;

pub struct SummaryServices {
    pub summary: Arc<SummaryService>,
    pub summary_refresh_handler: Arc<dyn TaskHandler>,
}

pub fn build_summary_services(ctx: &BootstrapContext<'_>) -> SummaryServices {
    let summary = Arc::new(SummaryService::new(
        Arc::clone(&ctx.repos.summary_repo),
        ctx.vector.vector_index.clone(),
    ));
    let summary_refresh_handler: Arc<dyn TaskHandler> = Arc::new(SummaryRefreshHandler::new(
        ctx.config.agent.enabled
            && ctx.config.agent.summary_enabled
            && ctx.config.agent.summary_async,
        Arc::clone(&ctx.infra.ollama_provider) as Arc<dyn LlmProvider>,
        Arc::clone(&ctx.repos.conv_repo) as Arc<dyn ConversationRepoT>,
        Arc::clone(&summary),
        Arc::clone(&ctx.repos.context_version_repo),
    ));

    SummaryServices {
        summary,
        summary_refresh_handler,
    }
}
