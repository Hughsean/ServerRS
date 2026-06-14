use thiserror::Error;
use validator::ValidationErrors;

#[derive(Debug, Error, Clone)]
pub enum AppError {
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
}

impl AppError {
    pub fn validation(err: ValidationErrors) -> Self {
        Self::Validation(err.to_string())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}
