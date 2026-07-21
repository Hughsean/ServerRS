use std::sync::Arc;

use crate::domain::risk::risk_repo::RiskRepoT;
use crate::shared::error::AppError;

#[derive(Debug, Clone)]
pub struct RiskStatsSummary {
    pub total: u64,
    pub trend: Vec<(String, u64)>,
    pub distribution: Vec<(String, u64)>,
}

pub struct RiskStatsService {
    repo: Arc<dyn RiskRepoT>,
}

impl RiskStatsService {
    pub fn new(repo: Arc<dyn RiskRepoT>) -> Self {
        Self { repo }
    }

    pub async fn summary(&self, days: u32) -> Result<RiskStatsSummary, AppError> {
        Ok(RiskStatsSummary {
            total: self.repo.count_all().await?,
            trend: self.repo.count_trend(days).await?,
            distribution: self.repo.count_by_risk_level().await?,
        })
    }
}
