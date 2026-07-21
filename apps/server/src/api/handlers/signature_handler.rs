use axum::{Json, extract::State};
use chrono::Utc;

use crate::api::dto::signature_dto::{
    SignatureCreateRequest, SignatureCreateResponse, SignatureVerifyRequest,
    SignatureVerifyResponse,
};
use crate::api::error::ApiError as AppError;
use crate::api::state::SignatureState;

/// POST /api/v1/signature/create
///
/// 使用 appKey 作为 HMAC 密钥签发 JWT 签名。
pub async fn create_signature(
    State(state): State<SignatureState>,
    Json(payload): Json<SignatureCreateRequest>,
) -> Result<Json<SignatureCreateResponse>, AppError> {
    let expires_in = payload.expires_in.unwrap_or(1800);

    let token =
        state
            .token_service
            .create_signature(&payload.app_id, &payload.app_key, expires_in)?;

    let now = Utc::now();
    let expires_at = now + chrono::Duration::seconds(expires_in);

    Ok(Json(SignatureCreateResponse {
        token,
        issued_at: now.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    }))
}

/// POST /api/v1/signature/verify
///
/// 使用 appKey 验证 JWT 签名是否有效。
pub async fn verify_signature(
    State(state): State<SignatureState>,
    Json(payload): Json<SignatureVerifyRequest>,
) -> Result<Json<SignatureVerifyResponse>, AppError> {
    let claims = state
        .token_service
        .verify_signature(&payload.token, &payload.app_key)?;

    Ok(Json(SignatureVerifyResponse {
        valid: claims.valid,
        app_id: claims.app_id,
        issued_at: claims.issued_at.map(|t| {
            let dt = chrono::DateTime::from_timestamp(t, 0).unwrap_or_default();
            dt.to_rfc3339()
        }),
        expires_at: claims.expires_at.map(|t| {
            let dt = chrono::DateTime::from_timestamp(t, 0).unwrap_or_default();
            dt.to_rfc3339()
        }),
    }))
}
