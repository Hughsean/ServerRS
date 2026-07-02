use std::sync::Arc;

use crate::app::web_ingestion::review_service::KnowledgeReviewService;
use crate::bootstrap::tasks::BackgroundTasks;
use crate::bootstrap::web_ingestion;

use super::BootstrapContext;

pub async fn build_web_ingestion_services(
    ctx: &BootstrapContext<'_>,
    background: &mut BackgroundTasks,
) -> Result<Arc<KnowledgeReviewService>, std::io::Error> {
    web_ingestion::init_web_ingestion(
        ctx.config,
        &ctx.infra.db,
        &ctx.vector.vector_store,
        &ctx.vector.embedding_provider,
        &ctx.repos.rag_repo,
        background,
    )
    .await
    .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error.to_string()))
}
