use std::sync::Arc;

use crate::app::summary::summary_service::SummaryService;

use super::BootstrapContext;

pub(crate) fn build_summary_service(ctx: &BootstrapContext<'_>) -> Arc<SummaryService> {
    Arc::new(SummaryService::new(
        Arc::clone(&ctx.repos.summary_repo),
        ctx.vector.vector_index.clone(),
    ))
}
