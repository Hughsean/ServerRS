use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};

use crate::api::DepressionState;
use crate::app::auth::auth_service::AuthenticatedUser;
use crate::app::depression::depression_service::AssessmentDetail;
use crate::shared::error::AppError;

// ── Request DTOs ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAssessmentRequest {
    pub scale_id: u16,
    pub answers: serde_json::Value,
    pub notes: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginationParams {
    #[serde(default = "default_page")]
    pub page: u64,
    #[serde(default = "default_size")]
    pub size: u64,
}

fn default_page() -> u64 {
    1
}
fn default_size() -> u64 {
    20
}

// ── Response DTOs ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScaleDto {
    pub scale_id: u16,
    pub scale_name: String,
    pub scale_description: Option<String>,
    pub min_score: i16,
    pub max_score: i16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentDto {
    pub assessment_id: u64,
    pub user_id: u64,
    pub scale_id: u16,
    pub assessment_date: String,
    pub answers: serde_json::Value,
    pub total_score: i16,
    /// Computed field — not persisted in the database.
    pub severity_level: String,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedAssessments {
    pub items: Vec<AssessmentDto>,
    pub total: u64,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// GET /api/v1/depression/scales
pub async fn list_scales(
    State(state): State<DepressionState>,
) -> Result<Json<Vec<ScaleDto>>, AppError> {
    let scales = state.depression.list_scales().await?;
    let dtos: Vec<ScaleDto> = scales
        .into_iter()
        .map(|s| ScaleDto {
            scale_id: s.scale_id,
            scale_name: s.scale_name,
            scale_description: s.scale_description,
            min_score: s.min_score,
            max_score: s.max_score,
        })
        .collect();
    Ok(Json(dtos))
}

/// GET /api/v1/depression/scales/{scale_id}
pub async fn get_scale(
    State(state): State<DepressionState>,
    Path(scale_id): Path<u16>,
) -> Result<Json<ScaleDto>, AppError> {
    let scale = state.depression.get_scale(scale_id).await?;
    Ok(Json(ScaleDto {
        scale_id: scale.scale_id,
        scale_name: scale.scale_name,
        scale_description: scale.scale_description,
        min_score: scale.min_score,
        max_score: scale.max_score,
    }))
}

/// GET /api/v1/depression/assessments
pub async fn list_assessments(
    State(state): State<DepressionState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<PaginatedAssessments>, AppError> {
    let (details, total) = state
        .depression
        .list_assessments(auth_user.user_id, params.page, params.size)
        .await?;
    let items: Vec<AssessmentDto> = details
        .into_iter()
        .map(|detail| {
            let a = detail.assessment;
            AssessmentDto {
                assessment_id: a.assessment_id,
                user_id: a.user_id,
                scale_id: a.scale_id,
                assessment_date: a.assessment_date.to_string(),
                answers: a.answers,
                total_score: a.total_score,
                severity_level: detail.severity_level,
                notes: a.notes,
                created_at: a.created_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                updated_at: a.updated_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
            }
        })
        .collect();
    Ok(Json(PaginatedAssessments { items, total }))
}

/// GET /api/v1/depression/assessments/{assessment_id}
pub async fn get_assessment(
    State(state): State<DepressionState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(assessment_id): Path<u64>,
) -> Result<Json<AssessmentDto>, AppError> {
    let detail = state
        .depression
        .get_assessment(auth_user.user_id, assessment_id)
        .await?;
    let a = detail.assessment;
    Ok(Json(AssessmentDto {
        assessment_id: a.assessment_id,
        user_id: a.user_id,
        scale_id: a.scale_id,
        assessment_date: a.assessment_date.to_string(),
        answers: a.answers,
        total_score: a.total_score,
        severity_level: detail.severity_level,
        notes: a.notes,
        created_at: a.created_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
        updated_at: a.updated_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
    }))
}

/// POST /api/v1/depression/assessments
pub async fn create_assessment(
    State(state): State<DepressionState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Json(payload): Json<CreateAssessmentRequest>,
) -> Result<Json<AssessmentDto>, AppError> {
    let AssessmentDetail {
        assessment,
        severity_level,
    } = state
        .depression
        .create_assessment(
            auth_user.user_id,
            payload.scale_id,
            payload.answers,
            payload.notes,
        )
        .await?;
    Ok(Json(AssessmentDto {
        assessment_id: assessment.assessment_id,
        user_id: assessment.user_id,
        scale_id: assessment.scale_id,
        assessment_date: assessment.assessment_date.to_string(),
        answers: assessment.answers,
        total_score: assessment.total_score,
        severity_level,
        notes: assessment.notes,
        created_at: assessment
            .created_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
        updated_at: assessment
            .updated_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
    }))
}

/// DELETE /api/v1/depression/assessments/{assessment_id}
pub async fn delete_assessment(
    State(state): State<DepressionState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(assessment_id): Path<u64>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .depression
        .delete_assessment(auth_user.user_id, assessment_id)
        .await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}
