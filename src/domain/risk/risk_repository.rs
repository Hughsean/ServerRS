use async_trait::async_trait;

use super::risk_detection_result::{NewRiskDetectionResult, RiskDetectionResult};
use crate::shared::error::AppError;

#[async_trait]
pub trait RiskRepository: Send + Sync {
    async fn save(&self, r: NewRiskDetectionResult) -> Result<RiskDetectionResult, AppError>;
    async fn find_by_user_id_paginated(
        &self,
        user_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<RiskDetectionResult>, u64), AppError>;
    async fn find_by_conversation_id(
        &self,
        conversation_id: u64,
    ) -> Result<Vec<RiskDetectionResult>, AppError>;
    async fn delete_by_conversation_id(&self, conversation_id: u64) -> Result<u64, AppError>;
}
