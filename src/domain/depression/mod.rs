use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::shared::error::AppError;

/// Matches depression_scales entity: scale_id (u16), scale_name, min_score, max_score.
/// created_at/updated_at are Option in the real DB.
/// There is no is_active column in the current database.
#[derive(Debug, Clone, Serialize)]
pub struct DepressionScale {
    pub scale_id: u16,
    pub scale_name: String,
    pub scale_description: Option<String>,
    pub min_score: i16,
    pub max_score: i16,
    pub questions: serde_json::Value,
    pub severity_ranges: serde_json::Value,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Matches depression_assessments entity: assessment_date is Date, total_score is i16.
/// There is NO severity_level column — it is computed in the service layer and returned
/// only via the response DTO.
#[derive(Debug, Clone, Serialize)]
pub struct DepressionAssessment {
    pub assessment_id: u64,
    pub user_id: u64,
    pub scale_id: u16,
    pub assessment_date: chrono::NaiveDate,
    pub answers: serde_json::Value,
    pub total_score: i16,
    pub notes: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewDepressionAssessment {
    pub user_id: u64,
    pub scale_id: u16,
    pub answers: serde_json::Value,
    pub notes: Option<String>,
}

#[async_trait]
pub trait DepressionRepository: Send + Sync {
    async fn find_scale_by_id(&self, id: u16) -> Result<Option<DepressionScale>, AppError>;
    async fn list_scales(&self) -> Result<Vec<DepressionScale>, AppError>;
    async fn save_assessment(
        &self,
        new: NewDepressionAssessment,
        total_score: i16,
    ) -> Result<DepressionAssessment, AppError>;
    async fn find_assessment_by_id(
        &self,
        id: u64,
    ) -> Result<Option<DepressionAssessment>, AppError>;
    async fn find_assessments_by_user_id(
        &self,
        user_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<DepressionAssessment>, u64), AppError>;
    async fn update_assessment(
        &self,
        id: u64,
        notes: Option<String>,
    ) -> Result<DepressionAssessment, AppError>;
    async fn delete_assessment(&self, id: u64) -> Result<u64, AppError>;
}
