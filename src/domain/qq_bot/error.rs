use std::fmt;

use crate::shared::error::AppError;

/// Domain-specific error type for QQ Bot operations.
#[derive(Debug)]
pub enum QqBotError {
    /// NapCat connection or communication failure.
    Connection(String),
    /// NapCat API returned an error code.
    Api {
        action: String,
        code: i32,
        message: String,
    },
    /// Message parsing / normalization failed.
    MessageProcessing(String),
    /// Requested entity was not found.
    NotFound(String),
    /// Internal / unexpected error.
    Internal(String),
}

impl fmt::Display for QqBotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(msg) => write!(f, "NapCat connection error: {msg}"),
            Self::Api { action, code, message } => {
                write!(f, "NapCat API error [{action}]: code={code}, {message}")
            }
            Self::MessageProcessing(msg) => write!(f, "message processing error: {msg}"),
            Self::NotFound(msg) => write!(f, "not found: {msg}"),
            Self::Internal(msg) => write!(f, "internal qq_bot error: {msg}"),
        }
    }
}

impl From<QqBotError> for AppError {
    fn from(e: QqBotError) -> Self {
        match e {
            QqBotError::Connection(msg) => AppError::Infrastructure(msg),
            QqBotError::Api { action, code, message } => {
                AppError::Internal(format!("NapCat API error [{action}]: code={code}, {message}"))
            }
            QqBotError::MessageProcessing(msg) => AppError::Validation(msg),
            QqBotError::NotFound(msg) => AppError::NotFound(msg),
            QqBotError::Internal(msg) => AppError::Internal(msg),
        }
    }
}
