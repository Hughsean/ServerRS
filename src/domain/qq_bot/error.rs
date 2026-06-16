use std::fmt;

use crate::shared::error::AppError;

/// QQ 机器人操作的领域特定错误类型。
#[derive(Debug)]
pub enum QqBotError {
    /// NapCat 连接或通信失败。
    Connection(String),
    /// NapCat API 返回了错误码。
    Api {
        action: String,
        code: i32,
        message: String,
    },
    /// 消息解析/标准化失败。
    MessageProcessing(String),
    /// 请求的实体未找到。
    NotFound(String),
    /// 内部/意外错误。
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
