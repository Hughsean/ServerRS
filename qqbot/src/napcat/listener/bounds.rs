//! 有界处理工具：WS 文本帧上限、raw_event 有界摘要、字节安全截断、actor ID 校验。
//!
//! 这些纯函数不依赖监听器实例状态，可独立测试。分层防护层级：
//! 1. `MAX_WS_TEXT_BYTES`：WS 文本帧在反序列化前检查（最外层）。
//! 2. `bound_raw_event`：raw_event 序列化大小超限时替换为有界摘要。
//! 3. `truncate_bytes`：raw_message 等字段在进入业务层前按字节截断。
//! 4. `validate_actor_ids`：身份字段在进入业务回调前校验。

use serde_json::Value;
use tracing::warn;

use super::super::MessageSegment;
use super::super::message_parser::parse_message_segments;
use super::super::segments::{MAX_MESSAGE_TOTAL_BYTES, parse_structured_segments};

/// 单条 OneBot WebSocket 文本帧字节上限（评审 P1-4）。
/// 超过此上限的帧在反序列化之前即被拒绝，防止无界 JSON 进入协议适配层。
/// 65536 字节足以容纳正常群/私聊消息与通知；巨型帧视为异常或攻击。
pub(crate) const MAX_WS_TEXT_BYTES: usize = 65_536;

/// 优先解析结构化 `message` 数组；不存在、非数组或为空时回退 CQ raw parser。
/// 结构化与等价 CQ 字符串生成等价的 canonical segment。
///
/// 评审 P1-3：CQ 回退前必须对 raw_message 做有界截断，否则无结构化数组的
/// 巨大 raw_message 会生成无界 Text segment 穿过协议边界。
pub(crate) fn parse_structured_or_cq(message: &Value, raw_message: &str) -> Vec<MessageSegment> {
    if let Value::Array(arr) = message
        && !arr.is_empty()
    {
        let (segments, truncated) = parse_structured_segments(arr);
        if truncated {
            warn!("消息段数量超过上限，已截断");
        }
        return segments;
    }
    // 结构化字段不存在或为空时才回退 CQ raw parser。
    // 先按字节上限截断 raw_message，保证 CQ 解析器不会生成无界 Text segment。
    let bounded_raw = truncate_bytes(raw_message, MAX_MESSAGE_TOTAL_BYTES);
    parse_message_segments(&bounded_raw)
}

/// 按字节截断到上限，保证多字节边界安全（不切割 UTF-8 字符）。
pub(crate) fn truncate_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

/// 把 raw_event 限制为有界大小（评审 P1-4）。
///
/// 序列化大小不超过 `MAX_WS_TEXT_BYTES` 时原样返回；超过时替换为只保留 `post_type`
/// 与截断原始文本的有界摘要，防止无界 `serde_json::Value` 穿过协议回调进入业务层
/// 与持久化。类型化字段（message_id/group_id/segments 等）已各自有界，不受影响。
pub(crate) fn bound_raw_event(raw_event: Value) -> Value {
    let serialized = match serde_json::to_string(&raw_event) {
        Ok(s) => s,
        // 序列化失败（理论上不应发生，Value 总可序列化）：替换为有界占位。
        Err(_) => {
            return serde_json::json!({"_bounded": "raw_event serialization failed"});
        }
    };
    if serialized.len() <= MAX_WS_TEXT_BYTES {
        return raw_event;
    }
    // 超限：替换为有界摘要。保留 post_type 供审计追溯；raw_text 截断到上限。
    let post_type = raw_event
        .get("post_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let bounded_text = truncate_bytes(&serialized, MAX_WS_TEXT_BYTES);
    tracing::warn!(
        original_bytes = serialized.len(),
        bound_bytes = MAX_WS_TEXT_BYTES,
        "raw_event 超过有界上限，已替换为截断摘要"
    );
    serde_json::json!({
        "post_type": post_type,
        "_bounded": true,
        "_original_bytes": serialized.len(),
        "raw_text": bounded_text,
    })
}

/// 校验群消息/通知的身份字段为正数。非正数视为协议错误，在进入业务回调前拒绝。
pub(crate) fn validate_actor_ids(
    group_id: i64,
    user_id: i64,
    event_name: &str,
) -> Result<(), super::super::NapCatError> {
    if group_id <= 0 || user_id <= 0 {
        return Err(super::super::NapCatError::Protocol(format!(
            "{event_name} requires positive group_id and user_id"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_bytes_is_char_boundary_safe() {
        // 多字节字符不应在中间被切割。
        let s = "你好世界你好世界";
        let bounded = truncate_bytes(s, 5);
        // 5 字节落在 "好" 中间（3 字节字符），回退到 3 字节边界。
        assert_eq!(bounded, "你");
        assert!(bounded.len() <= 5);
    }

    #[test]
    fn truncate_bytes_keeps_short_values_unchanged() {
        assert_eq!(truncate_bytes("abc", 100), "abc");
        assert_eq!(truncate_bytes("", 100), "");
    }

    #[test]
    fn bound_raw_event_passes_small_events_through() {
        let small = serde_json::json!({
            "post_type": "message",
            "message_id": "x",
            "raw_message": "hi"
        });
        let bounded = bound_raw_event(small.clone());
        // 小事件原样返回（未被替换）。
        assert_eq!(bounded.get("post_type"), small.get("post_type"));
        assert!(bounded.get("_bounded").is_none());
    }

    #[test]
    fn bound_raw_event_replaces_oversized_events() {
        let big_text = "x".repeat(MAX_WS_TEXT_BYTES + 1000);
        let big = serde_json::json!({
            "post_type": "message",
            "raw_message": big_text,
        });
        let bounded = bound_raw_event(big);
        assert_eq!(
            bounded.get("_bounded").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            bounded.get("post_type").and_then(|v| v.as_str()),
            Some("message")
        );
    }

    #[test]
    fn validate_actor_ids_rejects_non_positive() {
        assert!(validate_actor_ids(0, 1, "notice").is_err());
        assert!(validate_actor_ids(1, 0, "notice").is_err());
        assert!(validate_actor_ids(-1, 1, "notice").is_err());
        assert!(validate_actor_ids(1, 1, "notice").is_ok());
    }
}
