use std::sync::Arc;

use crate::app::risk::post_conversation_risk_audit_worker::PostConversationRiskAuditWorker;
use crate::app::risk::risk_detection_service::RiskDetectionService;
use crate::domain::risk::risk_detector::RiskDetector;
use crate::domain::tasks::task_handler::TaskHandler;
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::infra::detector::rule_based_detector::RuleBasedRiskDetector;

use super::BootstrapContext;

pub struct RiskServices {
    pub risk_audit_worker: Arc<dyn TaskHandler>,
}

pub fn build_risk_services(
    ctx: &BootstrapContext<'_>,
    task_publisher: Arc<dyn TaskPublisher>,
) -> RiskServices {
    let risk_detector: Arc<dyn RiskDetector> = Arc::new(RuleBasedRiskDetector::new());
    let risk_detection = Arc::new(RiskDetectionService::new(
        Arc::clone(&ctx.repos.risk_repo),
        task_publisher,
        risk_detector,
    ));
    let risk_audit_worker: Arc<dyn TaskHandler> = Arc::new(PostConversationRiskAuditWorker::new(
        Arc::clone(&ctx.repos.conv_repo),
        Arc::clone(&risk_detection),
    ));

    RiskServices { risk_audit_worker }
}
