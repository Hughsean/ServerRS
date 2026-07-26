//! 关闭信号来源：OS 信号或可编程 `watch` 通道。
//!
//! `watch::Receiver::changed()` 在任何值变化时返回（包括 `false`），必须循环检查
//! `borrow()` 只有 `true` 才表示关闭；忽略 `false` 变化（如 watch 初始化或误触发）。

use tokio::sync::watch;

/// 关闭信号来源：OS 信号或可编程 watch 通道。
pub(super) enum ShutdownSource {
    OsSignal,
    Watch(watch::Receiver<bool>),
}

impl ShutdownSource {
    /// 等待关闭信号触发。返回后调用方应开始优雅关闭。
    pub(super) async fn wait(&mut self) {
        match self {
            ShutdownSource::OsSignal => shutdown_signal().await,
            ShutdownSource::Watch(receiver) => {
                // 只在收到 true 时才关闭；忽略 false 变化（如 watch 初始化或其他误触发）。
                loop {
                    if *receiver.borrow() {
                        return;
                    }
                    // changed() 在值变化时返回；若 sender 被 drop 则返回 Err（视为关闭）。
                    if receiver.changed().await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut signal) = signal(SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
