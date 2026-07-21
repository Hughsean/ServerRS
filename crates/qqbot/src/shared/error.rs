use std::fmt;

/// QQ 机器人边界内统一使用的错误类型。
#[derive(Debug)]
pub enum QqBotError {
    Connection(String),
    Api {
        action: String,
        code: i32,
        message: String,
    },
    MessageProcessing(String),
    NotFound(String),
    Internal(String),
}

impl fmt::Display for QqBotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(message) => write!(f, "NapCat connection error: {message}"),
            Self::Api {
                action,
                code,
                message,
            } => write!(f, "NapCat API error [{action}]: code={code}, {message}"),
            Self::MessageProcessing(message) => {
                write!(f, "message processing error: {message}")
            }
            Self::NotFound(message) => write!(f, "not found: {message}"),
            Self::Internal(message) => write!(f, "internal qq_bot error: {message}"),
        }
    }
}

impl std::error::Error for QqBotError {}

pub type AppError = QqBotError;
