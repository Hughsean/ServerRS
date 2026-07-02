use std::sync::Arc;

use crate::app::summary::summary_service::SummaryService;
use crate::domain::tasks::task_handler::TaskHandler;

use super::{
    BootstrapContext, summary_handler_provider::build_summary_refresh_handler,
    summary_service_provider::build_summary_service,
};

pub struct SummaryServices {
    pub summary: Arc<SummaryService>,
    pub summary_refresh_handler: Arc<dyn TaskHandler>,
}

pub fn build_summary_services(ctx: &BootstrapContext<'_>) -> SummaryServices {
    let summary = build_summary_service(ctx);
    let summary_refresh_handler = build_summary_refresh_handler(ctx, Arc::clone(&summary));

    SummaryServices {
        summary,
        summary_refresh_handler,
    }
}
