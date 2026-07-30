use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use validator::ValidationErrors;

#[derive(Debug, thiserror::Error, Clone)]
pub enum ApiError {
    #[error("request validation failed: {0}")]
    Validation(String),
    #[error("authentication failed")]
    Unauthorized,
    #[error("access denied: {0}")]
    Forbidden(String),
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("infrastructure error: {0}")]
    Infrastructure(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("gone: {0}")]
    Gone(String),
}

impl ApiError {
    pub fn validation(error: ValidationErrors) -> Self {
        Self::Validation(error.to_string())
    }
}

impl From<digital_human::shared::error::AppError> for ApiError {
    fn from(error: digital_human::shared::error::AppError) -> Self {
        use digital_human::shared::error::AppError;

        match error {
            AppError::Validation(message) => Self::Validation(message),
            AppError::Unauthorized => Self::Unauthorized,
            AppError::Forbidden(message) => Self::Forbidden(message),
            AppError::NotFound(message) => Self::NotFound(message),
            AppError::Conflict(message) => Self::Conflict(message),
            AppError::Infrastructure(message) => Self::Infrastructure(message),
            AppError::Internal(message) => Self::Internal(message),
            AppError::NotImplemented(message) => Self::NotImplemented(message),
            AppError::Gone(message) => Self::Gone(message),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::Validation(_) => (StatusCode::BAD_REQUEST, "VALIDATION_ERROR"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED"),
            Self::Forbidden(_) => (StatusCode::FORBIDDEN, "FORBIDDEN"),
            Self::NotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "CONFLICT"),
            Self::Infrastructure(_) => (StatusCode::BAD_GATEWAY, "INFRASTRUCTURE_ERROR"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
            Self::NotImplemented(_) => (StatusCode::NOT_IMPLEMENTED, "NOT_IMPLEMENTED"),
            Self::Gone(_) => (StatusCode::GONE, "GONE"),
        };

        let log_context = ApiErrorLogContext {
            code,
            message: self.to_string(),
        };
        let body = Json(ErrorResponse {
            code,
            message: log_context.message.clone(),
        });
        let mut response = (status, body).into_response();
        response.extensions_mut().insert(log_context);
        response
    }
}

/// Internal response metadata consumed by the HTTP logging middleware. Keeping
/// it in extensions lets the middleware log the original error together with
/// the request method and URI without exposing extra fields in the API body.
#[derive(Debug, Clone)]
pub(crate) struct ApiErrorLogContext {
    pub code: &'static str,
    pub message: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    code: &'static str,
    message: String,
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;

    use super::{ApiError, ApiErrorLogContext};

    #[test]
    fn internal_error_preserves_original_detail_for_request_logging() {
        let response = ApiError::Internal("database exploded".into()).into_response();
        let context = response
            .extensions()
            .get::<ApiErrorLogContext>()
            .expect("ApiError response should carry logging context");

        assert_eq!(context.code, "INTERNAL_ERROR");
        assert_eq!(context.message, "internal error: database exploded");
    }
}
