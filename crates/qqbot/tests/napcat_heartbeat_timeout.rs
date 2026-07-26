//! 评审 P1-6：验证 `run_forward()` 在未收到 Heartbeat 时产生 `HeartbeatTimeout`。
//!
//! 本测试启动一个本地 WebSocket 服务器：接受连接后**不发送任何消息**（模拟
//! NapCat 不发送 meta_event/heartbeat 的场景）。监听器配置极短的 startup_grace
//! 与 interval，使 deadline 快速到期。断言 `run_forward()` 返回
//! `NapCatError::HeartbeatTimeout`，而非永久挂起或返回其他错误。
//!
//! 这是对 Heartbeat 状态机三态（Disabled/Waiting/Expired）在真实 WebSocket 传输
//! 层的端到端验证，不依赖实机 NapCat。

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use qqbot::napcat::{
    HeartbeatConfig, NapCatError, NapCatEvent, NapCatEventHandler, NapCatListener,
};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

/// 不做任何事的 handler：收到事件也不传播错误，确保只有 Heartbeat 超时能结束循环。
struct NoopHandler;

#[async_trait::async_trait]
impl NapCatEventHandler for NoopHandler {
    async fn handle(&self, _event: NapCatEvent) -> Result<(), NapCatError> {
        Ok(())
    }
}

/// 启动一个本地 WebSocket 服务器：接受连接后不发送任何消息，模拟无 Heartbeat 的 NapCat。
async fn start_silent_ws_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        // 接受 WebSocket 握手后保持连接但不发送任何消息。
        let mut ws = accept_async(stream).await.unwrap();
        // 等待客户端关闭或超时；不主动发送 heartbeat/lifecycle。
        // 这里读取客户端可能发送的任何消息（忽略），保持连接活着。
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(tokio_tungstenite::tungstenite::Message::Close(_)) | Err(_) => break,
                Ok(_) => { /* 忽略客户端消息 */ }
            }
        }
    });
    addr
}

#[tokio::test]
async fn run_forward_returns_heartbeat_timeout_when_no_heartbeat_received() {
    let addr = start_silent_ws_server().await;
    let ws_url = format!("ws://{addr}");

    // 配置极短的 startup_grace 与 interval，使 deadline 快速到期。
    // timeout_multiplier=1 + startup_grace=1s：未收到首个 heartbeat 时 1s 后超时。
    let config = HeartbeatConfig {
        enabled: true,
        startup_grace_secs: 1,
        min_interval_secs: 1,
        max_interval_secs: 5,
        default_interval_secs: 1,
        timeout_multiplier: 1,
    };

    let handler = std::sync::Arc::new(NoopHandler);
    let listener = NapCatListener::new(ws_url, 10001, handler).with_heartbeat_config(config);

    let result = tokio::time::timeout(Duration::from_secs(10), listener.run_forward()).await;

    // 关键断言：run_forward 在超时内返回（不永久挂起）。
    let result = result.expect("run_forward must return within 10s, not hang forever");
    // 返回 HeartbeatTimeout，而非 Connection/Protocol/Handler 错误。
    match result {
        Err(NapCatError::HeartbeatTimeout(_)) => { /* 预期：Heartbeat 超时 */ }
        other => panic!("expected NapCatError::HeartbeatTimeout, got {other:?}"),
    }
}

/// 验证 Heartbeat 监控禁用时 run_forward 不会因超时返回 HeartbeatTimeout。
/// 服务器发送 Close 帧后 run_forward 应正常返回 Ok(())。
#[tokio::test]
async fn run_forward_returns_ok_when_disabled_and_server_closes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        // 立即发送 Close 帧，结束连接。
        ws.send(tokio_tungstenite::tungstenite::Message::Close(None))
            .await
            .unwrap();
    });

    let ws_url = format!("ws://{addr}");
    let config = HeartbeatConfig {
        enabled: false,
        ..HeartbeatConfig::default()
    };
    let handler = std::sync::Arc::new(NoopHandler);
    let listener = NapCatListener::new(ws_url, 10001, handler).with_heartbeat_config(config);

    let result = tokio::time::timeout(Duration::from_secs(5), listener.run_forward())
        .await
        .expect("run_forward must return within 5s");
    assert!(
        result.is_ok(),
        "disabled heartbeat + server close should return Ok"
    );
}
