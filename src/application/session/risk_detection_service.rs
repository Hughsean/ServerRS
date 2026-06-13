use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::domain::risk::detection_types::{DetectionResult, RiskLevel};
use crate::domain::risk::risk_detection_result::NewRiskDetectionResult;
use crate::domain::risk::risk_detector::RiskDetector;
use crate::domain::risk::risk_repository::RiskRepository;
use crate::domain::tasks::task_event::{RiskDetectedTask, TaskEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;

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

    /// Persist a detection result and publish a `RiskDetected` task
    /// only for actionable risk levels (Low, Medium, High, Crisis).
    ///
    /// - None / Unknown → saved to DB but NOT published.
    /// - Crisis / High → warn-level log + published.
    /// - Low / Medium → info/debug log + published.
    ///
    /// Failures to save or publish are logged at `warn` and never panic.
    pub async fn persist_and_publish_result(
        &self,
        result: DetectionResult,
        user_id: u64,
        conversation_id: Option<u64>,
        message_id: Option<u64>,
    ) {
        let is_actionable =
            result.risk_level != RiskLevel::None && result.risk_level != RiskLevel::Unknown;

        if is_actionable {
            info!(
                user_id,
                ?conversation_id,
                risk_level = ?result.risk_level,
                confidence = result.confidence,
                "risk detected"
            );
        } else {
            debug!(
                user_id,
                ?conversation_id,
                risk_level = ?result.risk_level,
                confidence = result.confidence,
                "risk detection completed (non-actionable)"
            );
        }

        // ── Persist to risk_detection_results (always) ──
        let evidence_json = serde_json::to_string(&result.evidence).unwrap_or_else(|_| "[]".into());

        if let Err(e) = self
            .risk_repo
            .save(NewRiskDetectionResult {
                user_id,
                message_id,
                conversation_id,
                risk_level: result.risk_level,
                polarity: result.polarity,
                intent: result.intent,
                target: result.target,
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

        // ── Publish RiskDetected task (actionable levels only) ──
        if is_actionable {
            if let Err(e) = self
                .task_publisher
                .publish(TaskEvent::RiskDetected(RiskDetectedTask {
                    user_id,
                    conversation_id,
                    risk_level: result.risk_level,
                    confidence: result.confidence,
                }))
                .await
            {
                warn!(error = %e, "failed to publish risk detected task");
            }
        }
    }

    /// Run detection on text, persist, and publish.
    /// Convenience method that combines `evaluate` + `persist_and_publish_result`.
    ///
    /// Uses `spawn_blocking` to avoid blocking the async runtime for
    /// detectors that may perform heavier computation.
    pub async fn detect_and_save(
        &self,
        text: &str,
        user_id: u64,
        conversation_id: Option<u64>,
        message_id: Option<u64>,
    ) {
        let text_owned = text.to_string();
        let detector = Arc::clone(&self.detector);
        let result = tokio::task::spawn_blocking(move || detector.evaluate(&text_owned))
            .await
            .unwrap_or_else(|_| DetectionResult::unknown());

        self.persist_and_publish_result(result, user_id, conversation_id, message_id)
            .await;
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    use crate::domain::risk::detection_types::{
        DetectionResult, IntentLabel, Polarity, RiskLevel, TargetLabel,
    };
    use crate::domain::risk::risk_detection_result::RiskDetectionResult;
    use crate::shared::error::AppError;

    // ── Mock RiskDetector ──────────────────────────────────────────────

    struct MockDetector {
        risk_level: RiskLevel,
    }

    impl RiskDetector for MockDetector {
        fn evaluate(&self, _text: &str) -> DetectionResult {
            DetectionResult {
                risk_level: self.risk_level,
                intent: IntentLabel::Narrative,
                evidence: vec![],
                polarity: Polarity::Neutral,
                target: TargetLabel::Unknown,
                confidence: 0.5,
                reason: String::new(),
            }
        }
    }

    // ── Mock RiskRepository ────────────────────────────────────────────

    struct MockRiskRepo {
        saved: std::sync::Mutex<Vec<NewRiskDetectionResult>>,
    }

    impl MockRiskRepo {
        fn new() -> Self {
            Self {
                saved: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl RiskRepository for MockRiskRepo {
        async fn save(&self, r: NewRiskDetectionResult) -> Result<RiskDetectionResult, AppError> {
            self.saved.lock().unwrap().push(r);
            Ok(RiskDetectionResult {
                id: 1,
                user_id: 0,
                message_id: None,
                conversation_id: None,
                risk_level: RiskLevel::None,
                polarity: Polarity::Neutral,
                intent: IntentLabel::Narrative,
                target: TargetLabel::Unknown,
                confidence: 0.5,
                evidence: "[]".into(),
                reason: None,
                raw_payload: None,
                model_name: None,
                detector_version: None,
                is_processed: false,
                process_notes: None,
                created_at: chrono::Utc::now(),
            })
        }

        async fn find_by_user_id_paginated(
            &self,
            _: u64,
            _: u64,
            _: u64,
        ) -> Result<(Vec<RiskDetectionResult>, u64), AppError> {
            Ok((vec![], 0))
        }

        async fn find_by_conversation_id(
            &self,
            _: u64,
        ) -> Result<Vec<RiskDetectionResult>, AppError> {
            Ok(vec![])
        }

        async fn find_all_paginated(
            &self,
            _: u64,
            _: u64,
            _: Option<RiskLevel>,
        ) -> Result<(Vec<RiskDetectionResult>, u64), AppError> {
            Ok((vec![], 0))
        }

        async fn find_conversation_ids_paginated(
            &self,
            _: u64,
            _: u64,
            _: Option<RiskLevel>,
        ) -> Result<(Vec<u64>, u64), AppError> {
            Ok((vec![], 0))
        }

        async fn mark_processed(
            &self,
            _: u64,
            _: Option<String>,
        ) -> Result<RiskDetectionResult, AppError> {
            Err(AppError::Internal("mock".into()))
        }

        async fn delete_by_conversation_id(&self, _: u64) -> Result<u64, AppError> {
            Ok(0)
        }
    }

    // ── Mock TaskPublisher ─────────────────────────────────────────────

    #[derive(Default)]
    struct MockTaskPublisher {
        events: std::sync::Mutex<Vec<TaskEvent>>,
    }

    #[async_trait]
    impl TaskPublisher for MockTaskPublisher {
        async fn publish(&self, event: TaskEvent) -> Result<(), AppError> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    // ── Helpers ────────────────────────────────────────────────────────

    fn build_service(
        risk_level: RiskLevel,
    ) -> (
        Arc<MockRiskRepo>,
        Arc<MockTaskPublisher>,
        RiskDetectionService,
    ) {
        let repo = Arc::new(MockRiskRepo::new());
        let publisher = Arc::new(MockTaskPublisher::default());
        let detector = Arc::new(MockDetector { risk_level });
        let svc = RiskDetectionService::new(
            Arc::clone(&repo) as Arc<dyn RiskRepository>,
            Arc::clone(&publisher) as Arc<dyn TaskPublisher>,
            Arc::clone(&detector) as Arc<dyn RiskDetector>,
        );
        (repo, publisher, svc)
    }

    // ── Tests ──────────────────────────────────────────────────────────

    #[test]
    fn evaluate_returns_detection_result() {
        let (_repo, _pub, svc) = build_service(RiskLevel::High);
        let result = svc.evaluate("test text");
        assert_eq!(result.risk_level, RiskLevel::High);
        assert_eq!(result.confidence, 0.5);
    }

    #[tokio::test]
    async fn none_risk_is_saved_but_not_published() {
        let (repo, publisher, svc) = build_service(RiskLevel::None);
        svc.persist_and_publish_result(svc.evaluate("test"), 1, None, None)
            .await;

        assert_eq!(repo.saved.lock().unwrap().len(), 1);
        assert!(publisher.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_risk_is_saved_but_not_published() {
        let (repo, publisher, svc) = build_service(RiskLevel::Unknown);
        svc.persist_and_publish_result(svc.evaluate("test"), 1, None, None)
            .await;

        assert_eq!(repo.saved.lock().unwrap().len(), 1);
        assert!(publisher.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn high_risk_is_saved_and_published() {
        let (repo, publisher, svc) = build_service(RiskLevel::High);
        svc.persist_and_publish_result(svc.evaluate("test"), 1, None, None)
            .await;

        assert_eq!(repo.saved.lock().unwrap().len(), 1);
        let events = publisher.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            TaskEvent::RiskDetected(t) => assert_eq!(t.risk_level, RiskLevel::High),
            _ => panic!("expected RiskDetected"),
        }
    }

    #[tokio::test]
    async fn crisis_risk_is_saved_and_published() {
        let (repo, publisher, svc) = build_service(RiskLevel::Crisis);
        svc.persist_and_publish_result(svc.evaluate("test"), 1, None, None)
            .await;

        assert_eq!(repo.saved.lock().unwrap().len(), 1);
        let events = publisher.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            TaskEvent::RiskDetected(t) => assert_eq!(t.risk_level, RiskLevel::Crisis),
            _ => panic!("expected RiskDetected"),
        }
    }

    #[tokio::test]
    async fn low_risk_is_saved_and_published() {
        let (repo, publisher, svc) = build_service(RiskLevel::Low);
        svc.persist_and_publish_result(svc.evaluate("test"), 1, None, None)
            .await;

        assert_eq!(repo.saved.lock().unwrap().len(), 1);
        let events = publisher.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            TaskEvent::RiskDetected(t) => assert_eq!(t.risk_level, RiskLevel::Low),
            _ => panic!("expected RiskDetected"),
        }
    }

    #[tokio::test]
    async fn medium_risk_is_saved_and_published() {
        let (repo, publisher, svc) = build_service(RiskLevel::Medium);
        svc.persist_and_publish_result(svc.evaluate("test"), 1, None, None)
            .await;

        assert_eq!(repo.saved.lock().unwrap().len(), 1);
        let events = publisher.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            TaskEvent::RiskDetected(t) => assert_eq!(t.risk_level, RiskLevel::Medium),
            _ => panic!("expected RiskDetected"),
        }
    }

    #[tokio::test]
    async fn detect_and_save_uses_evaluate_and_persist() {
        let (repo, publisher, svc) = build_service(RiskLevel::High);
        svc.detect_and_save("some text", 42, Some(10), Some(100))
            .await;

        let saved = repo.saved.lock().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].user_id, 42);
        assert_eq!(saved[0].conversation_id, Some(10));
        assert_eq!(saved[0].message_id, Some(100));

        let events = publisher.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            TaskEvent::RiskDetected(t) => {
                assert_eq!(t.user_id, 42);
                assert_eq!(t.conversation_id, Some(10));
            }
            _ => panic!("expected RiskDetected"),
        }
    }
}
