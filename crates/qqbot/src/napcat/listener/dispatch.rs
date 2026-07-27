//! 事件分发：WS 文本帧 -> 有界检查 -> JSON 解析 -> meta_event/notice/message 路由。
//!
//! 分层防护：WS 文本帧字节上限在反序列化前检查（最外层）；raw_event 有界摘要
//! 在分发到具体 handler 前应用；meta_event 只更新 Heartbeat 监控状态，不进业务路径。

use serde_json::Value;
use tracing::debug;

use super::super::NapCatEventHandler;
use super::super::heartbeat::{HeartbeatState, parse_meta_event};
use super::bounds::{MAX_WS_TEXT_BYTES, bound_raw_event};
use super::message_event::handle_message;
use super::notice_event::handle_notice;

/// 处理一条 WS 文本消息：有界检查、JSON 解析、meta_event 过滤、业务事件分发。
///
/// 非 meta_event 视为业务事件：记录时间戳但不重置 Heartbeat deadline，
/// 避免普通文本流量掩盖已启用的 Heartbeat 超时。
pub(crate) async fn handle_ws_message(
    handler: &dyn NapCatEventHandler,
    self_qq_id: i64,
    text: &str,
    heartbeat: &mut HeartbeatState,
) -> Result<(), super::super::NapCatError> {
    // 评审 P1-4：在反序列化之前检查 WS 文本帧字节上限。
    // 超过上限的帧视为异常或攻击，直接拒绝，不进入协议适配层。
    if text.len() > MAX_WS_TEXT_BYTES {
        return Err(super::super::NapCatError::Protocol(format!(
            "WebSocket text frame exceeds {} bytes (got {}); rejected before deserialization",
            MAX_WS_TEXT_BYTES,
            text.len()
        )));
    }
    let raw_event: Value = serde_json::from_str(text)
        .map_err(|error| super::super::NapCatError::Protocol(format!("invalid JSON: {error}")))?;
    let post_type = raw_event
        .get("post_type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // meta_event（heartbeat/lifecycle）只更新监控状态，不进入业务路径、不持久化。
    if let Some(meta) = parse_meta_event(&raw_event) {
        heartbeat.observe_meta(&meta);
        return Ok(());
    }

    // 非 meta_event 视为业务事件：记录时间戳但不重置 Heartbeat deadline，
    // 避免普通文本流量掩盖已启用的 Heartbeat 超时。
    heartbeat.observe_business_event();

    // 评审 P1-4：raw_event 以无界 serde_json::Value 穿过协议回调。
    // 序列化大小超过上限时替换为有界摘要，只保留 post_type 与截断后的原始文本，
    // 防止无界 JSON 进入业务层与持久化。类型化字段（message_id/segments 等）已各自有界。
    let bounded_raw_event = bound_raw_event(raw_event);

    match post_type.as_str() {
        "notice" => handle_notice(handler, bounded_raw_event).await,
        "message" => handle_message(handler, self_qq_id, bounded_raw_event, false).await,
        "message_sent" => handle_message(handler, self_qq_id, bounded_raw_event, true).await,
        _ => {
            debug!(post_type = %post_type, "忽略尚未建模的 OneBot 事件");
            Ok(())
        }
    }
}
