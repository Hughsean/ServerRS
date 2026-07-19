use std::sync::Arc;

use crate::app::risk::risk_stats_service::RiskStatsService;
use crate::domain::tasks::task_handler::TaskHandler;
use crate::domain::tasks::task_publisher::TaskPublisher;

use super::{
    BootstrapContext, risk_audit_provider::build_risk_audit_worker,
    risk_detection_provider::build_risk_detection_service,
};

pub struct RiskServices {
    pub risk_audit_worker: Arc<dyn TaskHandler>,
    pub stats: Arc<RiskStatsService>,
}

pub fn build_risk_services(
    ctx: &BootstrapContext<'_>,
    task_publisher: Arc<dyn TaskPublisher>,
) -> RiskServices {
    let risk_detection = build_risk_detection_service(ctx, task_publisher);
    let risk_audit_worker = build_risk_audit_worker(ctx, risk_detection);
    let stats = Arc::new(RiskStatsService::new(Arc::clone(&ctx.repos.risk_repo)));

    RiskServices {
        risk_audit_worker,
        stats,
    }
}
