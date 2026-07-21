use std::sync::Arc;

use crate::app::risk::post_conversation_risk_audit_worker::PostConversationRiskAuditWorker;
use crate::app::risk::risk_detection_service::RiskDetectionService;
use crate::domain::tasks::task_handler::TaskHandler;

use super::BootstrapContext;

pub(crate) fn build_risk_audit_worker(
    ctx: &BootstrapContext<'_>,
    risk_detection: Arc<RiskDetectionService>,
) -> Arc<dyn TaskHandler> {
    Arc::new(PostConversationRiskAuditWorker::new(
        Arc::clone(&ctx.repos.conv_repo),
        risk_detection,
    ))
}
