//! Stable, machine-readable quality-gate result (task-book §7.3).
//!
//! The persisted `quality_result` JSON MUST NOT depend on a Rust `Debug`
//! string. `QualityCheckedHandler` reads `decision` from this stable schema and
//! never re-runs the gate.

use serde::{Deserialize, Serialize};

use crate::app::web_ingestion::quality_gate::QualityGateDecision;

/// Stable decision discriminator persisted in `knowledge_ingestion_runs.quality_result`.
pub mod decision {
    pub const REJECTED: &str = "rejected";
    pub const STAGED: &str = "staged";
    pub const PUBLISHABLE: &str = "publishable";
}

/// The gate version — bump when the gate logic changes.
pub const GATE_VERSION: &str = "20260613_v1";

/// Stable JSON schema for a quality-gate outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QualityResult {
    /// One of `decision::{REJECTED, STAGED, PUBLISHABLE}`.
    pub decision: String,
    pub reason: String,
    pub quality_score: f64,
    pub risk_flags: Vec<String>,
    /// Whether the gate permits publishing (true only for `publishable`).
    pub should_publish: bool,
    pub gate_version: String,
    /// RFC3339 timestamp.
    pub evaluated_at: String,
}

impl QualityResult {
    pub fn from_decision(
        decision: &QualityGateDecision,
        quality_score: f64,
        risk_flags: Vec<String>,
    ) -> Self {
        let (decision_str, reason, should_publish) = match decision {
            QualityGateDecision::Rejected { reason } => (decision::REJECTED, reason.clone(), false),
            QualityGateDecision::Staged { reason } => (decision::STAGED, reason.clone(), false),
            QualityGateDecision::Publishable => {
                (decision::PUBLISHABLE, "all checks passed".to_string(), true)
            }
        };
        Self {
            decision: decision_str.to_string(),
            reason,
            quality_score,
            risk_flags,
            should_publish,
            gate_version: GATE_VERSION.to_string(),
            evaluated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn is_rejected(&self) -> bool {
        self.decision == decision::REJECTED
    }

    pub fn is_publishable(&self) -> bool {
        self.decision == decision::PUBLISHABLE
    }

    /// Parse from a stored JSON value (lenient — defaults missing fields).
    pub fn from_json(v: &serde_json::Value) -> Self {
        serde_json::from_value(v.clone()).unwrap_or_else(|_| Self {
            decision: v["decision"].as_str().unwrap_or("rejected").to_string(),
            reason: v["reason"]
                .as_str()
                .unwrap_or("unparseable quality_result")
                .to_string(),
            quality_score: v["quality_score"].as_f64().unwrap_or(0.0),
            risk_flags: v["risk_flags"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            should_publish: v["should_publish"].as_bool().unwrap_or(false),
            gate_version: v["gate_version"].as_str().unwrap_or("unknown").to_string(),
            evaluated_at: v["evaluated_at"].as_str().unwrap_or("").to_string(),
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({"decision": "rejected"}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_roundtrip_no_debug_string() {
        let d = QualityGateDecision::Rejected {
            reason: "too short".into(),
        };
        let r = QualityResult::from_decision(&d, 0.1, vec![]);
        assert_eq!(r.decision, decision::REJECTED);
        assert!(!r.should_publish);
        let j = r.to_json();
        // Stable schema: a real field, not a "Rejected { reason: ... }" debug blob.
        assert_eq!(j["decision"], "rejected");
        let parsed = QualityResult::from_json(&j);
        assert_eq!(parsed, r);
    }

    #[test]
    fn publishable_sets_should_publish() {
        let r = QualityResult::from_decision(&QualityGateDecision::Publishable, 0.9, vec![]);
        assert!(r.is_publishable());
        assert!(r.should_publish);
    }

    #[test]
    fn staged_is_not_publishable() {
        let d = QualityGateDecision::Staged {
            reason: "manual review".into(),
        };
        let r = QualityResult::from_decision(&d, 0.8, vec!["legal_policy".into()]);
        assert_eq!(r.decision, decision::STAGED);
        assert!(!r.should_publish);
        assert!(!r.is_rejected());
    }
}
