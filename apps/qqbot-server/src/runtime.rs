use std::sync::Arc;
use std::time::Duration;

use qqbot::napcat::{NapCatError, NapCatEvent, NapCatEventHandler, NapCatListener};

use crate::config::AppConfig;

/// 在新业务接入前只观测协议事件，不回复、不持久化，也不修改任何外部状态。
struct PendingBusinessHandler;

#[async_trait::async_trait]
impl NapCatEventHandler for PendingBusinessHandler {
    async fn handle(&self, event: NapCatEvent) -> Result<(), NapCatError> {
        match event {
            NapCatEvent::GroupMessage(event) => tracing::info!(
                group_id = event.group_id,
                user_id = event.user_id,
                message_id = %event.message_id,
                "NapCat 群消息已接收；QQBot 业务尚未接入"
            ),
            NapCatEvent::GroupMemberIncrease(event) => tracing::info!(
                group_id = event.group_id,
                user_id = event.user_id,
                "NapCat 入群通知已接收；QQBot 业务尚未接入"
            ),
            NapCatEvent::GroupMemberDecrease(event) => tracing::info!(
                group_id = event.group_id,
                user_id = event.user_id,
                sub_type = %event.sub_type,
                "NapCat 退群通知已接收；QQBot 业务尚未接入"
            ),
            NapCatEvent::Poke(event) => tracing::info!(
                group_id = event.group_id,
                user_id = event.user_id,
                target_id = ?event.target_id,
                "NapCat 戳一戳通知已接收；QQBot 业务尚未接入"
            ),
        }
        Ok(())
    }
}

pub async fn run(config: AppConfig) {
    let handler: Arc<dyn NapCatEventHandler> = Arc::new(PendingBusinessHandler);
    let listener = NapCatListener::new(
        config.napcat.ws_url.clone(),
        config.napcat.self_qq_id,
        handler,
    );
    let mut backoff = config.napcat.reconnect_initial_secs;

    loop {
        tokio::select! {
            _ = shutdown_signal() => {
                tracing::info!("QQBot NapCat 适配器正在退出");
                return;
            }
            result = listener.run_forward() => {
                match result {
                    Ok(()) => tracing::warn!("NapCat WebSocket 已断开"),
                    Err(error) => tracing::warn!(error = %error, "NapCat WebSocket 运行失败"),
                }
            }
        }

        tracing::info!(backoff_secs = backoff, "等待后重新连接 NapCat");
        tokio::select! {
            _ = shutdown_signal() => return,
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
        }
        backoff = backoff
            .saturating_mul(2)
            .min(config.napcat.reconnect_max_secs);
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
