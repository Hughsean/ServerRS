use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskLevel {
    None,
    Low,
    Medium,
    High,
    Crisis,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Polarity {
    Positive,
    Neutral,
    Negative,
    Mixed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IntentLabel {
    HelpSeeking,
    Venting,
    InfoQuery,
    Narrative,
    JokeSarcasm,
    CrisisSelfHarm,
    ClarificationRequest,
    FollowUpQuestion,
    Opinion,
    ToxicAbuse,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TargetLabel {
    #[serde(rename = "SELF")]
    SelfTarget,
    #[serde(rename = "OTHER_INDIVIDUAL")]
    OtherIndividual,
    #[serde(rename = "GROUP_ORG")]
    GroupOrg,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectionResult {
    pub risk_level: RiskLevel,
    pub polarity: Polarity,
    pub intent: IntentLabel,
    pub target: TargetLabel,
    pub evidence: Vec<String>,
    pub confidence: f64,
    pub reason: String,
}

impl DetectionResult {
    pub fn unknown() -> Self {
        Self {
            risk_level: RiskLevel::Unknown,
            polarity: Polarity::Unknown,
            intent: IntentLabel::Unknown,
            target: TargetLabel::Unknown,
            evidence: vec![],
            confidence: 0.4,
            reason: String::new(),
        }
    }
}
