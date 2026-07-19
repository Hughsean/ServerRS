use axum::Json;
use axum::extract::State;

use crate::api::AdminState;
use crate::api::dto::stats_dto::{CountTrendResponse, RiskStatsResponse, StringCount};
use crate::shared::error::AppError;

/// GET /api/v1/admin/stats/users
pub async fn stats_users(
    State(state): State<AdminState>,
) -> Result<Json<CountTrendResponse>, AppError> {
    let total = state.user.count_all().await?;
    let trend = state.user.count_trend(7).await?;
    Ok(Json(CountTrendResponse {
        total,
        trend: trend
            .into_iter()
            .map(|(label, count)| StringCount { label, count })
            .collect(),
    }))
}

/// GET /api/v1/admin/stats/music
pub async fn stats_music(
    State(state): State<AdminState>,
) -> Result<Json<CountTrendResponse>, AppError> {
    let total = state.music.count_all().await?;
    let trend = state.music.count_trend(7).await?;
    Ok(Json(CountTrendResponse {
        total,
        trend: trend
            .into_iter()
            .map(|(label, count)| StringCount { label, count })
            .collect(),
    }))
}

/// GET /api/v1/admin/stats/reviews
pub async fn stats_reviews(
    State(state): State<AdminState>,
) -> Result<Json<CountTrendResponse>, AppError> {
    let total = state.knowledge_review.count_all().await?;
    let trend = state.knowledge_review.count_trend(7).await?;
    Ok(Json(CountTrendResponse {
        total,
        trend: trend
            .into_iter()
            .map(|(label, count)| StringCount { label, count })
            .collect(),
    }))
}

/// GET /api/v1/admin/stats/risks
pub async fn stats_risks(
    State(state): State<AdminState>,
) -> Result<Json<RiskStatsResponse>, AppError> {
    let summary = state.risk_stats.summary(7).await?;
    Ok(Json(RiskStatsResponse {
        total: summary.total,
        trend: summary
            .trend
            .into_iter()
            .map(|(label, count)| StringCount { label, count })
            .collect(),
        distribution: summary
            .distribution
            .into_iter()
            .map(|(label, count)| StringCount { label, count })
            .collect(),
    }))
}
