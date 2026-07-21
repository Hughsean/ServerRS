use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CountTrendResponse {
    pub total: u64,
    pub trend: Vec<StringCount>,
}

#[derive(Debug, Serialize)]
pub struct StringCount {
    pub label: String,
    pub count: u64,
}

#[derive(Debug, Serialize)]
pub struct RiskStatsResponse {
    pub total: u64,
    pub trend: Vec<StringCount>,
    pub distribution: Vec<StringCount>,
}
