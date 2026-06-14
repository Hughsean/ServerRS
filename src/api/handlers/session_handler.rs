use axum::Json;

use crate::api::dto::risk_dto::RiskDetectionPage;
use crate::shared::error::AppError;

/// GET /api/v1/risk-detections
/// Risk data is no longer exposed to end users in the post-conversation
/// audit model (design §4.1 / §6.3). Audits are internal/admin-only.
/// Returns an empty page to keep the route backward-compatible until removed.
pub async fn list_risk_detections() -> Result<Json<RiskDetectionPage>, AppError> {
    Ok(Json(RiskDetectionPage {
        items: Vec::new(),
        total: 0,
        page: 1,
        size: 10,
    }))
}
