//! 传输层：WebSocket 建连、单条帧读取、Ping/Pong/Close 处理。
//!
//! `run_forward` 在 `mod.rs` 中，因为它持有 `HeartbeatState` 三态 deadline 驱动循环；
//! 本模块只负责无 Heartbeat 抢占时的单条帧读取与协议帧分类（Text/Ping/Close/Error）。

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use super::super::heartbeat::HeartbeatState;
use super::super::{NapCatError, NapCatEventHandler};
use super::dispatch::handle_ws_message;

/// 监控禁用路径：接收一条 WS 消息并处理。返回 `Ok(true)` 表示连接已关闭。
pub(crate) async fn recv_once(
    handler: &dyn NapCatEventHandler,
    self_qq_id: i64,
    stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    heartbeat: &mut HeartbeatState,
) -> Result<bool, NapCatError> {
    let message = stream.next().await;
    handle_one_message(handler, self_qq_id, stream, message, heartbeat).await
}

/// 处理一条 WS 消息。返回 `Ok(true)` 表示连接已关闭（应结束循环）。
pub(crate) async fn handle_one_message(
    handler: &dyn NapCatEventHandler,
    self_qq_id: i64,
    stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    message: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    heartbeat: &mut HeartbeatState,
) -> Result<bool, NapCatError> {
    let Some(message) = message else {
        return Ok(true);
    };
    match message {
        Ok(Message::Text(text)) => {
            if let Err(error) =
                handle_ws_message(handler, self_qq_id, text.as_str(), heartbeat).await
            {
                warn!(error = %error, "NapCat 事件处理失败");
            }
        }
        Ok(Message::Ping(payload)) => {
            stream.send(Message::Pong(payload)).await.map_err(|error| {
                NapCatError::Connection(format!("WebSocket pong failed: {error}"))
            })?;
        }
        Ok(Message::Close(_)) => {
            info!("NapCat WebSocket 已关闭");
            return Ok(true);
        }
        Ok(_) => {}
        Err(error) => {
            return Err(NapCatError::Connection(format!(
                "WebSocket receive failed: {error}"
            )));
        }
    }
    Ok(false)
}
