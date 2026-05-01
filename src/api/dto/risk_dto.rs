use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct RiskDetectionResponse {
    pub id: u64,
    pub conversation_id: Option<u64>,
    pub risk_level: String,
    pub polarity: String,
    pub intent: String,
    pub reason: Option<String>,
    pub confidence: f64,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct RiskDetectionPage {
    pub items: Vec<RiskDetectionResponse>,
    pub total: u64,
    pub page: u64,
    pub size: u64,
}
