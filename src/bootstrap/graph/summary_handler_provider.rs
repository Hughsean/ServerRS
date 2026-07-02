use std::sync::Arc;

use crate::app::summary::summary_refresh_handler::SummaryRefreshHandler;
use crate::app::summary::summary_service::SummaryService;
use crate::domain::conversation::conversation_repository::ConversationRepoT;
use crate::domain::llm::LlmProvider;
use crate::domain::tasks::task_handler::TaskHandler;

use super::BootstrapContext;

pub(crate) fn build_summary_refresh_handler(
    ctx: &BootstrapContext<'_>,
    summary: Arc<SummaryService>,
) -> Arc<dyn TaskHandler> {
    Arc::new(SummaryRefreshHandler::new(
        ctx.config.agent.enabled
            && ctx.config.agent.summary_enabled
            && ctx.config.agent.summary_async,
        Arc::clone(&ctx.infra.ollama_provider) as Arc<dyn LlmProvider>,
        Arc::clone(&ctx.repos.conv_repo) as Arc<dyn ConversationRepoT>,
        summary,
        Arc::clone(&ctx.repos.context_version_repo),
    ))
}
