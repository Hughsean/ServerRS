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
    /// OneBot Heartbeat 超时：监听连接因长时间未收到 Heartbeat 而结束。
    /// 区别于网络关闭与协议错误，由宿主据此进入 Epoch/Gap/Backfill 链路。
    #[error("NapCat heartbeat timeout: {0}")]
    HeartbeatTimeout(String),
}
