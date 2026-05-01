use axum::{Json, extract::State};
use validator::Validate;

use crate::api::ApiState;
use crate::api::dto::auth_dto::{
    HealthResponse, LoginRequest, LoginResponse, LogoutRequest, LogoutResponse,
    RefreshTokenRequest, RefreshTokenResponse, RegisterRequest, RegisterResponse,
};
use crate::api::response::ApiResponse;
use crate::application::auth::auth_service::LoginInput;
use crate::shared::error::AppError;

pub async fn health() -> Json<ApiResponse<HealthResponse>> {
    Json(ApiResponse::ok(HealthResponse { status: "up" }))
}

pub async fn login(
    State(state): State<ApiState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<ApiResponse<LoginResponse>>, AppError> {
    payload.validate().map_err(AppError::validation)?;

    let result = state
        .auth
        .login(LoginInput {
            username: payload.username,
            password: payload.password,
            device_id: payload.device_id,
        })
        .await?;

    Ok(Json(ApiResponse::ok(LoginResponse {
        user_id: result.user_id,
        access_token: result.access_token,
        refresh_token: result.refresh_token,
    })))
}

pub async fn register(
    State(state): State<ApiState>,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<ApiResponse<RegisterResponse>>, AppError> {
    payload.validate().map_err(AppError::validation)?;

    let result = state
        .auth
        .register(payload.username, payload.password, payload.device_id)
        .await?;

    Ok(Json(ApiResponse::ok(RegisterResponse {
        user_id: result.user_id,
        access_token: result.access_token,
        refresh_token: result.refresh_token,
    })))
}

pub async fn refresh_token(
    State(state): State<ApiState>,
    Json(payload): Json<RefreshTokenRequest>,
) -> Result<Json<ApiResponse<RefreshTokenResponse>>, AppError> {
    payload.validate().map_err(AppError::validation)?;

    let result = state
        .auth
        .refresh(&payload.refresh_token, payload.device_id)
        .await?;

    Ok(Json(ApiResponse::ok(RefreshTokenResponse {
        access_token: result.access_token,
        refresh_token: result.refresh_token,
    })))
}

pub async fn logout(
    State(state): State<ApiState>,
    Json(payload): Json<LogoutRequest>,
) -> Result<Json<ApiResponse<LogoutResponse>>, AppError> {
    payload.validate().map_err(AppError::validation)?;

    let result = state
        .auth
        .logout(&payload.refresh_token, payload.reason)
        .await?;

    Ok(Json(ApiResponse::ok(LogoutResponse { success: result })))
}
