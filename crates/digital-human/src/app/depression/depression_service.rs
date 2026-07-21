use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::depression::{
    DepressionAssessment, DepressionRepoT, DepressionScale, NewDepressionAssessment,
};
use crate::shared::error::AppError;

pub struct DepressionService {
    pub repo: Arc<dyn DepressionRepoT>,
}

impl DepressionService {
    pub fn new(repo: Arc<dyn DepressionRepoT>) -> Self {
        Self { repo }
    }

    pub async fn list_scales(&self) -> Result<Vec<DepressionScale>, AppError> {
        self.repo.list_scales().await
    }

    pub async fn get_scale(&self, scale_id: u16) -> Result<DepressionScale, AppError> {
        self.repo
            .find_scale_by_id(scale_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("scale {scale_id} not found")))
    }

    /// Creates an assessment. Computes total_score from answers, determines severity_level
    /// from the scale's severity_ranges, and returns an AssessmentDetail DTO that includes
    /// severity_level as a computed field (NOT persisted in the database).
    pub async fn create_assessment(
        &self,
        user_id: u64,
        scale_id: u16,
        answers: serde_json::Value,
        notes: Option<String>,
    ) -> Result<AssessmentDetail, AppError> {
        let scale = self.get_scale(scale_id).await?;

        let total_score = compute_score(&answers)?;
        if total_score < scale.min_score as i32 || total_score > scale.max_score as i32 {
            return Err(AppError::Validation(format!(
                "assessment score {total_score} is outside scale range {}..={}",
                scale.min_score, scale.max_score
            )));
        }
        let severity_level = determine_severity(&scale.severity_ranges, total_score);

        let assessment = self
            .repo
            .save_assessment(
                NewDepressionAssessment {
                    user_id,
                    scale_id,
                    answers,
                    notes,
                },
                total_score as i16,
            )
            .await?;

        Ok(AssessmentDetail {
            assessment,
            severity_level,
        })
    }

    pub async fn list_assessments(
        &self,
        user_id: u64,
        page: u64,
        size: u64,
    ) -> Result<(Vec<AssessmentDetail>, u64), AppError> {
        let page = page.max(1);
        let size = size.clamp(1, 100);
        let offset = page.saturating_sub(1) * size;
        let (assessments, total) = self
            .repo
            .find_assessments_by_user_id(user_id, size, offset)
            .await?;
        let scales: HashMap<u16, DepressionScale> = self
            .repo
            .list_scales()
            .await?
            .into_iter()
            .map(|scale| (scale.scale_id, scale))
            .collect();
        let items = assessments
            .into_iter()
            .map(|assessment| {
                let severity_level = scales
                    .get(&assessment.scale_id)
                    .map(|scale| {
                        determine_severity(&scale.severity_ranges, assessment.total_score as i32)
                    })
                    .unwrap_or_else(|| "unknown".to_string());
                AssessmentDetail {
                    assessment,
                    severity_level,
                }
            })
            .collect();
        Ok((items, total))
    }

    pub async fn get_assessment(
        &self,
        user_id: u64,
        assessment_id: u64,
    ) -> Result<AssessmentDetail, AppError> {
        let a = self
            .repo
            .find_assessment_by_id(assessment_id)
            .await?
            .ok_or_else(|| AppError::NotFound("assessment not found".into()))?;
        if a.user_id != user_id {
            return Err(AppError::Forbidden("not your assessment".into()));
        }
        let scale = self.get_scale(a.scale_id).await?;
        let severity_level = determine_severity(&scale.severity_ranges, a.total_score as i32);
        Ok(AssessmentDetail {
            assessment: a,
            severity_level,
        })
    }

    pub async fn delete_assessment(
        &self,
        user_id: u64,
        assessment_id: u64,
    ) -> Result<(), AppError> {
        let a = self
            .repo
            .find_assessment_by_id(assessment_id)
            .await?
            .ok_or_else(|| AppError::NotFound("assessment not found".into()))?;
        if a.user_id != user_id {
            return Err(AppError::Forbidden("not your assessment".into()));
        }
        self.repo.delete_assessment(assessment_id).await?;
        Ok(())
    }
}

/// DTO for assessment creation response — includes computed severity_level that
/// is NOT persisted in the depression_assessments table.
pub struct AssessmentDetail {
    pub assessment: DepressionAssessment,
    pub severity_level: String,
}

fn compute_score(answers: &serde_json::Value) -> Result<i32, AppError> {
    let values: Vec<&serde_json::Value> = match answers {
        serde_json::Value::Object(map) if !map.is_empty() => map.values().collect(),
        serde_json::Value::Array(arr) if !arr.is_empty() => arr.iter().collect(),
        _ => {
            return Err(AppError::Validation(
                "answers must be a non-empty object or array".into(),
            ));
        }
    };

    values.into_iter().try_fold(0_i32, |total, value| {
        let score = value.as_i64().ok_or_else(|| {
            AppError::Validation("every assessment answer must be an integer".into())
        })?;
        let score = i32::try_from(score)
            .map_err(|_| AppError::Validation("assessment answer is out of range".into()))?;
        total
            .checked_add(score)
            .ok_or_else(|| AppError::Validation("assessment score overflow".into()))
    })
}

fn determine_severity(severity_ranges: &serde_json::Value, score: i32) -> String {
    if let Some(ranges) = severity_ranges.as_array() {
        for range in ranges {
            let min = range
                .get("min")
                .and_then(|v| v.as_i64())
                .unwrap_or(i64::MIN);
            let max = range
                .get("max")
                .and_then(|v| v.as_i64())
                .unwrap_or(i64::MAX);
            if score >= min as i32 && score <= max as i32 {
                if let Some(label) = range
                    .get("level")
                    .or_else(|| range.get("label"))
                    .and_then(|v| v.as_str())
                {
                    return label.to_string();
                }
            }
        }
    }
    "unknown".to_string()
}
