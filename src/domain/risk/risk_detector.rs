use super::detection_types::DetectionResult;

/// 风险检测端口 — 基础设施提供实际的检测器。
pub trait RiskDetector: Send + Sync {
    fn evaluate(&self, text: &str) -> DetectionResult;
}
