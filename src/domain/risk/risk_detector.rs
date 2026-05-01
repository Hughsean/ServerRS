use super::detection_types::DetectionResult;

/// Port for risk detection — infrastructure provides the real detector.
pub trait RiskDetector: Send + Sync {
    fn evaluate(&self, text: &str) -> DetectionResult;
}
