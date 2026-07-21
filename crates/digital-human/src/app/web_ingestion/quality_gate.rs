//! Quality gate — enforces hard rules beyond what DeepSeek self-reports.
//!
//! Rules (task-book §10):
//! - clean_text too short → rejected
//! - distilled.accept = false → rejected
//! - sections empty → rejected
//! - summary empty → rejected
//! - quality_score < 0.65 → rejected
//! - source.approval_status != approved → rejected
//! - quality_score < auto_publish_min_score → staged
//! - source.auto_publish = false → staged
//! - staging_required = true → staged
//! - high-risk flags present → staged
//! - unknown risk flag → staged

use crate::domain::web_ingestion::error::WebIngestionError;

/// High-risk flags that block auto-publish.
const HIGH_RISK_FLAGS: &[&str] = &[
    "self_harm_crisis",
    "medication_dosage",
    "diagnosis_criteria",
    "medical_claim",
    "legal_policy",
    "financial_advice",
    "minors_high_risk",
    "emergency_advice",
];

/// Quality gate decision.
#[derive(Debug, Clone, PartialEq)]
pub enum QualityGateDecision {
    Rejected { reason: String },
    Staged { reason: String },
    Publishable,
}

/// Input to the quality gate.
#[derive(Debug, Clone)]
pub struct QualityGateInput {
    pub clean_text: String,
    pub distilled_accept: bool,
    pub distilled_summary: String,
    pub distilled_sections_count: usize,
    pub distilled_quality_score: f64,
    pub distilled_risk_flags: Vec<String>,
    pub source_approval_status: String,
    pub source_auto_publish: bool,
    pub source_trust_level: String,
    pub staging_required: bool,
    pub auto_publish_min_score: f64,
}

/// Evaluate the quality gate.
pub fn evaluate(input: &QualityGateInput) -> Result<QualityGateDecision, WebIngestionError> {
    use crate::domain::web_ingestion::status::source_approval;

    // ── Hard reject rules ──────────────────────────────────────────

    // Clean text too short (less than 100 characters is useless)
    if input.clean_text.chars().count() < 100 {
        return Ok(QualityGateDecision::Rejected {
            reason: "clean text too short (< 100 chars)".into(),
        });
    }

    // DeepSeek said no
    if !input.distilled_accept {
        return Ok(QualityGateDecision::Rejected {
            reason: "distilled.accept = false".into(),
        });
    }

    // No sections extracted
    if input.distilled_sections_count == 0 {
        return Ok(QualityGateDecision::Rejected {
            reason: "distilled sections is empty".into(),
        });
    }

    // Summary is empty
    if input.distilled_summary.trim().is_empty() {
        return Ok(QualityGateDecision::Rejected {
            reason: "distilled summary is empty".into(),
        });
    }

    // Quality score too low
    if input.distilled_quality_score < 0.65 {
        return Ok(QualityGateDecision::Rejected {
            reason: format!("quality_score {} < 0.65", input.distilled_quality_score),
        });
    }

    // Source not approved
    if input.source_approval_status != source_approval::APPROVED {
        return Ok(QualityGateDecision::Rejected {
            reason: format!("source approval_status = {}", input.source_approval_status),
        });
    }

    // ── Staging vs auto-publish rules ─────────────────────────────

    // Any high-risk flag → staged
    for flag in &input.distilled_risk_flags {
        if HIGH_RISK_FLAGS.contains(&flag.as_str()) {
            return Ok(QualityGateDecision::Staged {
                reason: format!("high-risk flag present: {flag}"),
            });
        }
        // Check for unknown risk flags
        if !is_known_risk_flag(flag) {
            return Ok(QualityGateDecision::Staged {
                reason: format!("unknown risk flag: {flag}"),
            });
        }
    }

    // Short-circuit to staged based on config
    if input.staging_required {
        return Ok(QualityGateDecision::Staged {
            reason: "web_ingestion.staging_required = true".into(),
        });
    }

    if !input.source_auto_publish {
        return Ok(QualityGateDecision::Staged {
            reason: "source.auto_publish = false".into(),
        });
    }

    if input.distilled_quality_score < input.auto_publish_min_score {
        return Ok(QualityGateDecision::Staged {
            reason: format!(
                "quality_score {} < auto_publish_min_score {}",
                input.distilled_quality_score, input.auto_publish_min_score
            ),
        });
    }

    // Non-official source with any risk flags → staged
    if input.source_trust_level != "official" && !input.distilled_risk_flags.is_empty() {
        return Ok(QualityGateDecision::Staged {
            reason: "non-official source with risk flags".into(),
        });
    }

    // ── All checks passed → publishable ───────────────────────────
    Ok(QualityGateDecision::Publishable)
}

/// Known risk flags (both safe and high-risk).
fn is_known_risk_flag(flag: &str) -> bool {
    matches!(
        flag,
        "self_harm_crisis"
            | "medication_dosage"
            | "diagnosis_criteria"
            | "medical_claim"
            | "legal_policy"
            | "financial_advice"
            | "minors_high_risk"
            | "emergency_advice"
            | "general_health"
            | "mental_health_low_risk"
            | "lifestyle_advice"
            | "general_info"
            | "educational"
            | "research_citation"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input() -> QualityGateInput {
        QualityGateInput {
            clean_text: "This is a sufficiently long clean text that contains enough characters to pass the minimum length requirement of the quality gate.".into(),
            distilled_accept: true,
            distilled_summary: "A useful summary".into(),
            distilled_sections_count: 3,
            distilled_quality_score: 0.85,
            distilled_risk_flags: vec![],
            source_approval_status: "approved".into(),
            source_auto_publish: true,
            source_trust_level: "official".into(),
            staging_required: false,
            auto_publish_min_score: 0.85,
        }
    }

    #[test]
    fn test_publishable() {
        let result = evaluate(&base_input()).unwrap();
        assert_eq!(result, QualityGateDecision::Publishable);
    }

    #[test]
    fn test_rejected_short_text() {
        let mut input = base_input();
        input.clean_text = "short".into();
        assert!(matches!(
            evaluate(&input).unwrap(),
            QualityGateDecision::Rejected { .. }
        ));
    }

    #[test]
    fn test_rejected_low_quality() {
        let mut input = base_input();
        input.distilled_quality_score = 0.5;
        assert!(matches!(
            evaluate(&input).unwrap(),
            QualityGateDecision::Rejected { .. }
        ));
    }

    #[test]
    fn test_staged_high_risk() {
        let mut input = base_input();
        input.distilled_risk_flags = vec!["self_harm_crisis".into()];
        assert!(matches!(
            evaluate(&input).unwrap(),
            QualityGateDecision::Staged { .. }
        ));
    }

    #[test]
    fn test_staged_unknown_risk() {
        let mut input = base_input();
        input.distilled_risk_flags = vec!["mystery_flag".into()];
        assert!(matches!(
            evaluate(&input).unwrap(),
            QualityGateDecision::Staged { .. }
        ));
    }

    #[test]
    fn test_staged_config_required() {
        let mut input = base_input();
        input.staging_required = true;
        assert!(matches!(
            evaluate(&input).unwrap(),
            QualityGateDecision::Staged { .. }
        ));
    }
}
