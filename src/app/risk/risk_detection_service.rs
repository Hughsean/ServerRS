use std::sync::Arc;

use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::domain::risk::detection_types::RiskLevel;
use crate::domain::risk::post_conversation_risk_audit::{
    NewPostConversationRiskAudit, PostRiskAuditResult,
};
use crate::domain::risk::risk_detector::RiskDetector;
use crate::domain::risk::risk_repository::RiskRepository;
use crate::domain::tasks::task_event::{RiskDetectedTask, TaskEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::shared::error::AppError;

/// Runs risk detection only for already-persisted, closed conversation turns.
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

    pub async fn audit_closed_turn(
        &self,
        user_id: u64,
        conversation_id: u64,
        user_message_id: u64,
        assistant_message_id: u64,
        canonical_input: String,
    ) -> Result<(), AppError> {
        let audit = self
            .risk_repo
            .create_pending(NewPostConversationRiskAudit {
                user_id,
                conversation_id,
                audit_scope: "turn".into(),
                user_message_ref_id: Some(user_message_id),
                assistant_message_ref_id: Some(assistant_message_id),
                user_message_id: Some(user_message_id),
                assistant_message_id: Some(assistant_message_id),
            })
            .await?;
        self.risk_repo.mark_running(audit.audit_id).await?;

        let detector = Arc::clone(&self.detector);
        let detector_input = canonical_input.clone();
        let result =
            match tokio::task::spawn_blocking(move || detector.evaluate(&detector_input)).await {
                Ok(result) => result,
                Err(error) => {
                    let message = format!("risk detector task failed: {error}");
                    self.risk_repo
                        .mark_failed(audit.audit_id, message.clone())
                        .await?;
                    return Err(AppError::internal(message));
                }
            };

        let input_hash = Sha256::digest(canonical_input.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let risk_level = serde_json::to_string(&result.risk_level)
            .unwrap_or_else(|_| "\"unknown\"".into())
            .trim_matches('"')
            .to_lowercase();
        self.risk_repo
            .mark_completed(
                audit.audit_id,
                PostRiskAuditResult {
                    risk_level,
                    risk_categories: None,
                    confidence: Some(result.confidence),
                    input_hash: Some(input_hash),
                    detector_name: Some("rule-based".into()),
                    detector_version: Some("1.0".into()),
                    model_name: None,
                    checked_at: chrono::Utc::now(),
                },
            )
            .await?;

        let actionable =
            result.risk_level != RiskLevel::None && result.risk_level != RiskLevel::Unknown;
        if actionable {
            info!(
                user_id,
                conversation_id,
                risk_level = ?result.risk_level,
                confidence = result.confidence,
                "post-conversation risk audit detected an actionable level"
            );
            if let Err(error) = self
                .task_publisher
                .publish(TaskEvent::RiskDetected(RiskDetectedTask {
                    user_id,
                    conversation_id: Some(conversation_id),
                    risk_level: result.risk_level,
                    confidence: result.confidence,
                }))
                .await
            {
                warn!(
                    user_id,
                    conversation_id,
                    %error,
                    "risk audit completed, but the follow-up notification could not be published"
                );
            }
        } else {
            debug!(
                user_id,
                conversation_id,
                risk_level = ?result.risk_level,
                "post-conversation risk audit completed"
            );
        }

        Ok(())
    }
}
