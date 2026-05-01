use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::domain::risk::detection_types::RiskLevel;
use crate::domain::risk::risk_detection_result::NewRiskDetectionResult;
use crate::domain::risk::risk_repository::RiskRepository;
use crate::domain::tasks::task_event::{RiskDetectedTask, TaskEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::infrastructure::detector::rule_based_detector::RuleBasedRiskDetector;

pub struct RiskDetectionService {
    risk_repo: Arc<dyn RiskRepository>,
    task_publisher: Arc<dyn TaskPublisher>,
}

impl RiskDetectionService {
    pub fn new(risk_repo: Arc<dyn RiskRepository>, task_publisher: Arc<dyn TaskPublisher>) -> Self {
        Self {
            risk_repo,
            task_publisher,
        }
    }

    /// Run rule-based detection on text and persist the result.
    /// Publishes a RiskDetected event through the task system.
    pub async fn detect_and_save(
        &self,
        text: &str,
        user_id: u64,
        conversation_id: Option<u64>,
        message_id: Option<u64>,
    ) {
        let text_owned = text.to_string();
        let result = tokio::task::spawn_blocking(move || {
            let detector = RuleBasedRiskDetector::new();
            detector.evaluate(&text_owned)
        })
        .await
        .unwrap_or_else(|_| crate::domain::risk::detection_types::DetectionResult::unknown());

        let risk_level_str = format!("{:?}", result.risk_level);

        if result.risk_level != RiskLevel::None && result.risk_level != RiskLevel::Unknown {
            info!(
                risk_level = ?result.risk_level,
                confidence = result.confidence,
                intent = ?result.intent,
                "risk detected"
            );
        } else {
            debug!(risk_level = ?result.risk_level, "risk detection completed");
        }

        // Publish event through unified task system
        let _ = self
            .task_publisher
            .publish(TaskEvent::RiskDetected(RiskDetectedTask {
                user_id,
                conversation_id,
                risk_level: risk_level_str,
                confidence: result.confidence,
            }))
            .await;

        let evidence_json = serde_json::to_string(&result.evidence).unwrap_or_else(|_| "[]".into());

        if let Err(e) = self
            .risk_repo
            .save(NewRiskDetectionResult {
                user_id,
                message_id,
                conversation_id,
                risk_level: format!("{:?}", result.risk_level),
                polarity: format!("{:?}", result.polarity),
                intent: format!("{:?}", result.intent),
                target: format!("{:?}", result.target),
                confidence: result.confidence,
                evidence: evidence_json,
                reason: if result.reason.is_empty() {
                    None
                } else {
                    Some(result.reason)
                },
                raw_payload: None,
                model_name: Some("rule-based".into()),
                detector_version: Some("1.0".into()),
            })
            .await
        {
            warn!(error = %e, "failed to persist risk detection");
        }
    }
}
