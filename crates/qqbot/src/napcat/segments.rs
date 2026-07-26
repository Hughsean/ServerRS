//! OneBot 结构化消息段解析器。
//!
//! 规范要点（B2）：
//! - 优先解析结构化 `message` 数组；结构化字段不存在或确实无法解析时才回退 CQ。
//! - 不允许裸 `serde_json::Value` 穿透协议适配层进入业务层。
//! - 未知段保留类型名与有界原始 JSON，不静默删除、不存无限大小。
//! - ID 字段兼容字符串和数字，进入内部后转换成明确类型。
//! - 设置段数量、单段文本、URL/文件名等上限；超长截断而非报错。
//! - 结构化数组与等价 CQ 字符串应生成等价的 canonical segment。
//!
//! 段类型定义在 [`super::event::MessageSegment`]；CQ 回退解析在
//! [`super::message_parser`]。本模块只负责结构化数组的类型化归一。

use super::event::{MessageSegment, RichKind};
use serde::Deserialize;

/// 单段文本字符上限，超出截断（约束：有界摘录）。
pub const MAX_SEGMENT_TEXT_CHARS: usize = 4_000;
/// 单条消息段数量上限，超出截断。
pub const MAX_SEGMENTS: usize = 64;
/// 单段 URL/文件名等元数据字符串上限。
pub const MAX_META_CHARS: usize = 2_000;
/// 单条消息序列化总字节上限（有界 envelope）。
/// 评审 P1-4：必须严格小于 `MAX_WS_TEXT_BYTES`，使 raw_message 审计截断层级独立于
/// WS 文本帧上限。raw_message 是 WS 帧的子字段，其上限应留出帧头与其它字段的空间。
pub const MAX_MESSAGE_TOTAL_BYTES: usize = 32_768;

/// 结构化段在 OneBot 中的原始项：`{"type": "...", "data": {...}}`。
#[derive(Debug, Deserialize)]
pub(crate) struct OneBotSegment {
    #[serde(rename = "type")]
    pub(crate) seg_type: String,
    #[serde(default)]
    pub(crate) data: serde_json::Map<String, serde_json::Value>,
}

/// 解析 OneBot 结构化 `message` 数组为类型化段。
///
/// - 逐段解析失败时该段降级为有界 `Unknown`，不中止整条消息。
/// - 强制段数量上限：超出 `MAX_SEGMENTS` 的段被截断。
/// - 返回 `(segments, truncated)`：`truncated=true` 表示因上限截断。
pub fn parse_structured_segments(message: &[serde_json::Value]) -> (Vec<MessageSegment>, bool) {
    let mut segments = Vec::with_capacity(message.len().min(MAX_SEGMENTS));
    let mut truncated = false;
    let mut total_bytes: usize = 0;
    for item in message {
        if segments.len() >= MAX_SEGMENTS {
            truncated = true;
            break;
        }
        // 评审 P1-1：在加入新段之前检查「加入后」的总字节数，而不是旧总量。
        // 旧实现检查 `total_bytes > limit` 会在超限后才截断，导致返回结果可能超过上限。
        // 先估算单段字节数，若加入后会超限则立即截断，保证返回结果严格有界。
        match serde_json::from_value::<OneBotSegment>(item.clone()) {
            Ok(seg) => {
                let seg = normalize_structured_segment(seg);
                let seg_bytes = segment_estimated_bytes(&seg);
                let next_total = total_bytes.saturating_add(seg_bytes);
                if next_total > MAX_MESSAGE_TOTAL_BYTES {
                    truncated = true;
                    break;
                }
                total_bytes = next_total;
                segments.push(seg);
            }
            Err(_) => {
                // 无法解析结构化项时保留为有界 Unknown，不静默删除。
                let raw = bounded_json_string(item);
                let raw_bytes = raw.as_ref().map(|r| r.len()).unwrap_or(0);
                let next_total = total_bytes.saturating_add(raw_bytes);
                if next_total > MAX_MESSAGE_TOTAL_BYTES {
                    truncated = true;
                    break;
                }
                total_bytes = next_total;
                segments.push(MessageSegment::Unknown {
                    seg_type: "?".into(),
                    raw,
                });
            }
        }
    }
    (segments, truncated)
}

/// 估算单段序列化后的字节数（粗略上限，用于总字节预算，避免精确序列化开销）。
fn segment_estimated_bytes(seg: &MessageSegment) -> usize {
    use MessageSegment::*;
    match seg {
        Text { content } => content.len(),
        At { qq } => qq.len() + 4,
        Reply { id } => id.len() + 4,
        Face { id: _, text } => 4 + text.as_ref().map(|t| t.len()).unwrap_or(0),
        Image { file, url } => file.len() + url.as_ref().map(|u| u.len()).unwrap_or(0),
        Record { file } => file.len(),
        Video { file, url } => file.len() + url.as_ref().map(|u| u.len()).unwrap_or(0),
        File {
            file,
            name,
            size: _,
        } => file.len() + name.as_ref().map(|n| n.len()).unwrap_or(0),
        Forward { id } => id.len(),
        Rich { data, summary, .. } => {
            data.as_ref().map(|d| d.len()).unwrap_or(0)
                + summary.as_ref().map(|s| s.len()).unwrap_or(0)
        }
        Unknown { seg_type, raw } => seg_type.len() + raw.as_ref().map(|r| r.len()).unwrap_or(0),
    }
}

/// 把单个结构化段归一化为类型化段；未知类型保留类型名与有界元数据。
fn normalize_structured_segment(seg: OneBotSegment) -> MessageSegment {
    let OneBotSegment { seg_type, data } = seg;
    match seg_type.as_str() {
        "text" => MessageSegment::Text {
            content: take_bounded_string(&data, "text", MAX_SEGMENT_TEXT_CHARS),
        },
        "at" => MessageSegment::At {
            qq: take_id_string(&data, "qq"),
        },
        "reply" => MessageSegment::Reply {
            id: take_id_string(&data, "id"),
        },
        "face" => MessageSegment::Face {
            id: take_i32(&data, "id").unwrap_or(0),
            text: take_opt_bounded_string(&data, "text", MAX_META_CHARS),
        },
        "image" => MessageSegment::Image {
            file: take_bounded_string(&data, "file", MAX_META_CHARS),
            url: take_opt_bounded_string(&data, "url", MAX_META_CHARS),
        },
        "record" => MessageSegment::Record {
            file: take_bounded_string(&data, "file", MAX_META_CHARS),
        },
        "video" => MessageSegment::Video {
            file: take_bounded_string(&data, "file", MAX_META_CHARS),
            url: take_opt_bounded_string(&data, "url", MAX_META_CHARS),
        },
        "file" => MessageSegment::File {
            file: take_bounded_string(&data, "file", MAX_META_CHARS),
            name: take_opt_bounded_string(&data, "name", MAX_META_CHARS),
            size: take_u64(&data, "size"),
        },
        "forward" => MessageSegment::Forward {
            id: take_id_string(&data, "id"),
        },
        "json" => MessageSegment::Rich {
            kind: RichKind::Json,
            data: take_opt_bounded_string(&data, "data", MAX_META_CHARS),
            summary: take_opt_bounded_string(&data, "summary", MAX_META_CHARS),
        },
        "xml" => MessageSegment::Rich {
            kind: RichKind::Xml,
            data: take_opt_bounded_string(&data, "data", MAX_META_CHARS),
            summary: take_opt_bounded_string(&data, "summary", MAX_META_CHARS),
        },
        "card" => MessageSegment::Rich {
            kind: RichKind::Card,
            data: take_opt_bounded_string(&data, "data", MAX_META_CHARS),
            summary: take_opt_bounded_string(&data, "summary", MAX_META_CHARS),
        },
        other => {
            // 未知段：保留类型名与有界原始 JSON，不存无限大小。
            let raw = serde_json::to_string(&serde_json::Value::Object(data))
                .ok()
                .map(|s| truncate_chars(&s, MAX_META_CHARS));
            MessageSegment::Unknown {
                seg_type: other.to_string(),
                raw,
            }
        }
    }
}

/// 取字段为字符串；OneBot 中 id/qq 等可能是数字或字符串，统一转成字符串。
fn take_id_string(data: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    match data.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn take_bounded_string(
    data: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    max_chars: usize,
) -> String {
    match data.get(key) {
        Some(serde_json::Value::String(s)) => truncate_chars(s, max_chars),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(other) => truncate_chars(&other.to_string(), max_chars),
        None => String::new(),
    }
}

fn take_opt_bounded_string(
    data: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    max_chars: usize,
) -> Option<String> {
    match data.get(key)? {
        serde_json::Value::String(s) => {
            let t = truncate_chars(s, max_chars);
            if t.is_empty() { None } else { Some(t) }
        }
        serde_json::Value::Number(n) => Some(n.to_string()),
        other => {
            let t = truncate_chars(&other.to_string(), max_chars);
            if t.is_empty() { None } else { Some(t) }
        }
    }
}

fn take_i32(data: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<i32> {
    match data.get(key)? {
        serde_json::Value::Number(n) => n.as_i64().and_then(|v| i32::try_from(v).ok()),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn take_u64(data: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<u64> {
    match data.get(key)? {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// 按字符数截断，保证多字节内容稳定有界。
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

/// 把任意 JSON Value 序列化为有界字符串，超长截断。
fn bounded_json_string(value: &serde_json::Value) -> Option<String> {
    serde_json::to_string(value)
        .ok()
        .map(|s| truncate_chars(&s, MAX_META_CHARS))
}

/// 从结构化段生成 canonical 文本（B2/P0-2，参考上游 getMsgRawTxt 思想）。
///
/// 结构化段是语义事实来源；`raw_message` 仅在结构化缺失时回退。
/// 文本段保留正文；非文本段用有界占位符（如 `[图片]`）表示，不内联媒体载荷。
/// 总字符数受 `MAX_SEGMENT_TEXT_CHARS` 上限约束（截断而非报错）。
pub fn segments_to_canonical_text(segments: &[MessageSegment]) -> String {
    let mut text = String::new();
    for seg in segments {
        match seg {
            MessageSegment::Text { content } => {
                text.push_str(&content.replace(['\n', '\r'], " "));
            }
            MessageSegment::At { qq } => {
                // @某人 用 `@qq` 占位（与上游显示昵称不同，协议适配层只保留可追溯 ID）。
                let _ = qq;
                text.push_str("@user");
            }
            MessageSegment::Reply { .. } => {
                // Reply 段不产生可见正文（上游也不显示），仅保留 ID 关联。
            }
            MessageSegment::Face { .. } => text.push_str("[表情]"),
            MessageSegment::Image { .. } => text.push_str("[图片]"),
            MessageSegment::Record { .. } => text.push_str("[语音]"),
            MessageSegment::Video { .. } => text.push_str("[视频]"),
            MessageSegment::File { name, .. } => {
                text.push_str("[文件]");
                if let Some(name) = name {
                    text.push_str(name);
                }
            }
            MessageSegment::Forward { .. } => text.push_str("[聊天记录]"),
            MessageSegment::Rich { kind, summary, .. } => {
                if let Some(s) = summary {
                    text.push_str(s);
                } else {
                    text.push_str(match kind {
                        RichKind::Json => "[卡片消息]",
                        RichKind::Xml => "[XML消息]",
                        RichKind::Card => "[卡片]",
                        RichKind::Other => "[富消息]",
                    });
                }
            }
            MessageSegment::Unknown { seg_type, .. } => {
                text.push('[');
                text.push_str(seg_type);
                text.push(']');
            }
        }
        if text.chars().count() > MAX_SEGMENT_TEXT_CHARS {
            return truncate_chars(&text, MAX_SEGMENT_TEXT_CHARS);
        }
    }
    text.trim().to_string()
}

/// 从结构化段提取 @ 目标（qq 列表）。用于 `at_bot` 判断与 mention 投影（P0-2）。
pub fn segments_mention_targets(segments: &[MessageSegment]) -> Vec<String> {
    segments
        .iter()
        .filter_map(|seg| match seg {
            MessageSegment::At { qq } => Some(qq.clone()),
            _ => None,
        })
        .collect()
}

/// 判断结构化段是否 @ 了指定账号（P0-2：at_bot 从结构化段得出，而非 raw_message）。
pub fn segments_mention_self(segments: &[MessageSegment], self_qq_id: i64) -> bool {
    segments.iter().any(|seg| match seg {
        MessageSegment::At { qq } => qq.parse::<i64>().is_ok_and(|id| id == self_qq_id),
        _ => false,
    })
}

/// 从结构化段提取 reply 的消息 ID（P0-2）。
pub fn segments_reply_id(segments: &[MessageSegment]) -> Option<String> {
    segments.iter().find_map(|seg| match seg {
        MessageSegment::Reply { id } => {
            if id.is_empty() {
                None
            } else {
                Some(id.clone())
            }
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_text_and_at_parse_to_canonical_segments() {
        let message = serde_json::json!([
            {"type":"at","data":{"qq":"10001"}},
            {"type":"text","data":{"text":"你好"}}
        ]);
        let (segs, trunc) = parse_structured_segments(message.as_array().unwrap());
        assert!(!trunc);
        assert!(matches!(segs[0], MessageSegment::At { ref qq } if qq == "10001"));
        assert!(matches!(segs[1], MessageSegment::Text { ref content } if content == "你好"));
    }

    #[test]
    fn numeric_id_is_coerced_to_string() {
        let message = serde_json::json!([
            {"type":"at","data":{"qq":10001}},
            {"type":"reply","data":{"id":42}}
        ]);
        let (segs, _) = parse_structured_segments(message.as_array().unwrap());
        assert!(matches!(segs[0], MessageSegment::At { ref qq } if qq == "10001"));
        assert!(matches!(segs[1], MessageSegment::Reply { ref id } if id == "42"));
    }

    #[test]
    fn image_video_file_forward_preserve_bounded_meta() {
        let message = serde_json::json!([
            {"type":"image","data":{"file":"a.jpg","url":"https://e.com/a.jpg"}},
            {"type":"video","data":{"file":"v.mp4"}},
            {"type":"file","data":{"file":"d.zip","name":"data","size":12345}},
            {"type":"forward","data":{"id":"fwd-1"}}
        ]);
        let (segs, _) = parse_structured_segments(message.as_array().unwrap());
        assert!(matches!(segs[0], MessageSegment::Image { .. }));
        assert!(matches!(segs[1], MessageSegment::Video { .. }));
        if let MessageSegment::File { size, .. } = &segs[2] {
            assert_eq!(*size, Some(12345));
        } else {
            panic!("expected File");
        }
        assert!(matches!(segs[3], MessageSegment::Forward { ref id } if id == "fwd-1"));
    }

    #[test]
    fn rich_json_xml_card_envelope_is_bounded() {
        let message = serde_json::json!([
            {"type":"json","data":{"data":"{}","summary":"card"}},
            {"type":"xml","data":{"data":"<xml/>"}}
        ]);
        let (segs, _) = parse_structured_segments(message.as_array().unwrap());
        assert!(matches!(
            segs[0],
            MessageSegment::Rich {
                kind: RichKind::Json,
                ..
            }
        ));
        assert!(matches!(
            segs[1],
            MessageSegment::Rich {
                kind: RichKind::Xml,
                ..
            }
        ));
    }

    #[test]
    fn unknown_segment_preserves_type_and_bounded_raw() {
        let message = serde_json::json!([
            {"type":"poke","data":{"name":"戳一戳","count":3}}
        ]);
        let (segs, _) = parse_structured_segments(message.as_array().unwrap());
        match &segs[0] {
            MessageSegment::Unknown { seg_type, raw } => {
                assert_eq!(seg_type, "poke");
                assert!(raw.is_some());
            }
            _ => panic!("expected Unknown"),
        }
    }

    #[test]
    fn unparseable_segment_becomes_bounded_unknown_not_dropped() {
        let message = serde_json::json!([
            {"not_a_segment": true},
            {"type":"text","data":{"text":"ok"}}
        ]);
        let (segs, _) = parse_structured_segments(message.as_array().unwrap());
        assert_eq!(segs.len(), 2);
        assert!(matches!(segs[0], MessageSegment::Unknown { .. }));
        assert!(matches!(segs[1], MessageSegment::Text { .. }));
    }

    #[test]
    fn segment_count_over_limit_is_truncated() {
        let mut arr = Vec::new();
        for _ in 0..(MAX_SEGMENTS + 5) {
            arr.push(serde_json::json!({"type":"text","data":{"text":"t"}}));
        }
        let (segs, trunc) = parse_structured_segments(&arr);
        assert_eq!(segs.len(), MAX_SEGMENTS);
        assert!(trunc);
    }

    #[test]
    fn oversized_text_is_truncated_by_chars() {
        // 仅由 'x' 组成，无需 JSON 转义；直接构造 Value 避免嵌套 format!。
        let big = "x".repeat(MAX_SEGMENT_TEXT_CHARS + 100);
        let mut data = serde_json::Map::new();
        data.insert("text".into(), serde_json::Value::String(big));
        let mut seg = serde_json::Map::new();
        seg.insert("type".into(), serde_json::Value::String("text".into()));
        seg.insert("data".into(), serde_json::Value::Object(data));
        let message = serde_json::Value::Array(vec![serde_json::Value::Object(seg)]);
        let (segs, _) = parse_structured_segments(message.as_array().unwrap());
        if let MessageSegment::Text { content } = &segs[0] {
            assert_eq!(content.chars().count(), MAX_SEGMENT_TEXT_CHARS);
        } else {
            panic!("expected a text segment here");
        }
    }

    // 评审 P1-1：总字节数预算必须在加入新段「之前」检查，保证返回结果严格有界。
    // 构造多个大文本段，使累计字节数超过 MAX_MESSAGE_TOTAL_BYTES，
    // 验证截断后所有已加入段的总字节数不超过上限。
    #[test]
    fn total_bytes_budget_is_enforced_before_adding_segment() {
        // 每段约 4000 字符 = 4000 字节（ASCII 'x'）。
        // MAX_MESSAGE_TOTAL_BYTES = 65536，约 16 段后超限。
        let per_segment_chars = MAX_SEGMENT_TEXT_CHARS;
        let segment_count = (MAX_MESSAGE_TOTAL_BYTES / per_segment_chars) + 4;
        let big = "x".repeat(per_segment_chars);
        let mut arr = Vec::new();
        for _ in 0..segment_count {
            let mut data = serde_json::Map::new();
            data.insert("text".into(), serde_json::Value::String(big.clone()));
            let mut seg = serde_json::Map::new();
            seg.insert("type".into(), serde_json::Value::String("text".into()));
            seg.insert("data".into(), serde_json::Value::Object(data));
            arr.push(serde_json::Value::Object(seg));
        }
        let (segs, truncated) = parse_structured_segments(&arr);
        assert!(
            truncated,
            "expected truncation when total bytes exceed budget"
        );
        // 关键断言：所有已加入段的文本字节数之和严格不超过上限。
        let total_bytes: usize = segs
            .iter()
            .map(|seg| match seg {
                MessageSegment::Text { content } => content.len(),
                _ => 0,
            })
            .sum();
        assert!(
            total_bytes <= MAX_MESSAGE_TOTAL_BYTES,
            "total bytes {total_bytes} exceeds limit {MAX_MESSAGE_TOTAL_BYTES}"
        );
    }
}
