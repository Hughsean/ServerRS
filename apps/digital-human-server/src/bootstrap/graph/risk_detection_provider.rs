use std::sync::Arc;

use crate::app::risk::risk_detection_service::RiskDetectionService;
use crate::domain::risk::risk_detector::RiskDetector;
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::infra::detector::rule_based_detector::RuleBasedRiskDetector;

use super::BootstrapContext;

pub(crate) fn build_risk_detection_service(
    ctx: &BootstrapContext<'_>,
    task_publisher: Arc<dyn TaskPublisher>,
) -> Arc<RiskDetectionService> {
    let risk_detector: Arc<dyn RiskDetector> = Arc::new(RuleBasedRiskDetector::new());
    Arc::new(RiskDetectionService::new(
        Arc::clone(&ctx.repos.risk_repo),
        task_publisher,
        risk_detector,
    ))
}
