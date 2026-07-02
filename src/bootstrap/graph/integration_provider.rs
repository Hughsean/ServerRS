use std::sync::Arc;

use crate::app::web_ingestion::review_service::KnowledgeReviewService;
use crate::bootstrap::tasks::BackgroundTasks;

use super::BootstrapContext;
use super::fresh_context_provider::init_fresh_context_integration;
use super::qq_bot_provider::init_qq_bot_integration;
use super::web_ingestion_provider::build_web_ingestion_services;

pub struct IntegrationServices {
    pub knowledge_review: Arc<KnowledgeReviewService>,
}

pub async fn build_integration_services(
    ctx: &BootstrapContext<'_>,
    background: &mut BackgroundTasks,
) -> Result<IntegrationServices, std::io::Error> {
    init_qq_bot_integration(ctx, background).await;
    let knowledge_review = build_web_ingestion_services(ctx, background).await?;
    init_fresh_context_integration(ctx, background).await?;

    Ok(IntegrationServices { knowledge_review })
}
