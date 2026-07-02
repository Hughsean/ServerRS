use std::sync::Arc;

use crate::domain::tasks::task_handler::TaskHandler;
use crate::domain::tasks::task_publisher::TaskPublisher;

use super::{
    BootstrapContext, risk_audit_provider::build_risk_audit_worker,
    risk_detection_provider::build_risk_detection_service,
};

pub struct RiskServices {
    pub risk_audit_worker: Arc<dyn TaskHandler>,
}

pub fn build_risk_services(
    ctx: &BootstrapContext<'_>,
    task_publisher: Arc<dyn TaskPublisher>,
) -> RiskServices {
    let risk_detection = build_risk_detection_service(ctx, task_publisher);
    let risk_audit_worker = build_risk_audit_worker(ctx, risk_detection);

    RiskServices { risk_audit_worker }
}
