use std::sync::Arc;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::app::web_ingestion::review_service::KnowledgeReviewService;
use crate::bootstrap::tasks::BackgroundTasks;

use super::BootstrapContext;
use super::fresh_context_provider::init_fresh_context_integration;
use super::web_ingestion_provider::build_web_ingestion_services;

pub struct IntegrationServices {
    pub knowledge_review: Arc<KnowledgeReviewService>,
    pub dispatcher_handle: Option<JoinHandle<()>>,
}

pub async fn build_integration_services(
    ctx: &BootstrapContext<'_>,
    background: &mut BackgroundTasks,
    shutdown_token: CancellationToken,
) -> Result<IntegrationServices, std::io::Error> {
    let (knowledge_review, dispatcher_handle) =
        build_web_ingestion_services(ctx, background, shutdown_token).await?;
    init_fresh_context_integration(ctx, background).await?;

    Ok(IntegrationServices {
        knowledge_review,
        dispatcher_handle,
    })
}
