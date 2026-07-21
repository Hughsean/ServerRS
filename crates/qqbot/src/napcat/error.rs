use thiserror::Error;

/// NapCat 传输与协议层错误。
#[derive(Debug, Error)]
pub enum NapCatError {
    #[error("NapCat connection error: {0}")]
    Connection(String),
    #[error("NapCat API error [{action}]: code={code}, {message}")]
    Api {
        action: String,
        code: i32,
        message: String,
    },
    #[error("NapCat protocol error: {0}")]
    Protocol(String),
    #[error("NapCat event handler error: {0}")]
    Handler(String),
}
