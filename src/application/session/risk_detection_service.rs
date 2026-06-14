use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::domain::risk::detection_types::{DetectionResult, RiskLevel};
use crate::domain::risk::post_conversation_risk_audit::{
    NewPostConversationRiskAudit, PostRiskAuditResult,
};
use crate::domain::risk::risk_detector::RiskDetector;
use crate::domain::risk::risk_repository::RiskRepository;
use crate::domain::tasks::task_event::{RiskDetectedTask, TaskEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;

/// Service that runs post-conversation risk detection and persists the result
/// as a completed `post_conversation_risk_audit`.
///
/// Per the post-risk architecture (design §2.3 / §6), this is invoked **only**
/// after a chat turn closes — never in the conversation generation path. The
/// ChatService / AgentRuntime / PromptBuilder never call into this service and
/// never read the resulting audits.
pub struct RiskDetectionService {
    risk_repo: Arc<dyn RiskRepository>,
    task_publisher: Arc<dyn TaskPublisher>,
    detector: Arc<dyn RiskDetector>,
}

impl RiskDetectionService {
    pub fn new(
        risk_repo: Arc<dyn RiskRepository>,
        task_publisher: Arc<dyn TaskPublisher>,
        detector: Arc<dyn RiskDetector>,
    ) -> Self {
        Self {
            risk_repo,
            task_publisher,
            detector,
        }
    }

    /// Run detection synchronously on text and return the result.
    /// Does NOT persist to the database or publish any task event.
    pub fn evaluate(&self, text: &str) -> DetectionResult {
        self.detector.evaluate(text)
    }

    /// Persist a detection result as a completed `post_conversation_risk_audit`
    /// and publish a `RiskDetected` task for actionable levels.
    ///
    /// - None / Unknown → audit saved but NOT published.
    /// - Crisis / High → warn-level log + published.
    /// - Low / Medium → info/debug log + published.
    ///
    /// Failures are logged at `warn` and never panic.
    pub async fn persist_and_publish_result(
        &self,
        result: DetectionResult,
        user_id: u64,
        conversation_id: u64,
        user_message_id: Option<u64>,
        assistant_message_id: Option<u64>,
    ) {
        let is_actionable =
            result.risk_level != RiskLevel::None && result.risk_level != RiskLevel::Unknown;

        if is_actionable {
            info!(
                user_id,
                conversation_id,
                risk_level = ?result.risk_level,
                confidence = result.confidence,
                "risk audit: actionable level detected"
            );
        } else {
            debug!(
                user_id,
                conversation_id,
                risk_level = ?result.risk_level,
                confidence = result.confidence,
                "risk audit: non-actionable"
            );
        }

        // ── Persist as a completed post-conversation risk audit ──
        let risk_level_str = serde_json::to_string(&result.risk_level)
            .unwrap_or_default()
            .trim_matches('"')
            .to_lowercase();

        match self
            .risk_repo
            .create_pending(NewPostConversationRiskAudit {
                user_id,
                conversation_id,
                audit_scope: "turn".to_string(),
                user_message_ref_id: user_message_id,
                assistant_message_ref_id: assistant_message_id,
                user_message_id,
                assistant_message_id,
            })
            .await
        {
            Ok(audit) => {
                let completed = self
                    .risk_repo
                    .mark_completed(
                        audit.audit_id,
                        PostRiskAuditResult {
                            risk_level: risk_level_str.clone(),
                            risk_categories: None,
                            confidence: Some(result.confidence),
                            input_hash: None,
                            detector_name: Some("rule-based".to_string()),
                            detector_version: Some("1.0".to_string()),
                            model_name: None,
                            checked_at: chrono::Utc::now(),
                        },
                    )
                    .await;
                if let Err(e) = completed {
                    warn!(error = %e, audit_id = audit.audit_id, "failed to complete risk audit");
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to create pending risk audit");
            }
        }

        // ── Publish RiskDetected task (actionable levels only) ──
        if is_actionable {
            if let Err(e) = self
                .task_publisher
                .publish(TaskEvent::RiskDetected(RiskDetectedTask {
                    user_id,
                    conversation_id: Some(conversation_id),
                    risk_level: result.risk_level,
                    confidence: result.confidence,
                }))
                .await
            {
                warn!(error = %e, "failed to publish risk detected task");
            }
        }
    }

    /// Run detection on text, persist as a completed audit, and publish.
    /// Uses `spawn_blocking` to avoid blocking the async runtime for
    /// detectors that may perform heavier computation.
    pub async fn detect_and_save(
        &self,
        text: &str,
        user_id: u64,
        conversation_id: u64,
        user_message_id: Option<u64>,
        assistant_message_id: Option<u64>,
    ) {
        let text_owned = text.to_string();
        let detector = Arc::clone(&self.detector);
        let result = tokio::task::spawn_blocking(move || detector.evaluate(&text_owned))
            .await
            .unwrap_or_else(|_| DetectionResult::unknown());

        self.persist_and_publish_result(
            result,
            user_id,
            conversation_id,
            user_message_id,
            assistant_message_id,
        )
        .await;
    }
}
