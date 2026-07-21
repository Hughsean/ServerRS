use super::MessageSegment;

/// Parse a raw message string from OneBot into segments.
/// Handles CQ codes like [CQ:at,qq=123] and [CQ:image,file=xxx].
pub fn parse_message_segments(raw: &str) -> Vec<MessageSegment> {
    if raw.contains("[CQ:") {
        parse_cq_codes(raw)
    } else {
        vec![MessageSegment::Text {
            content: raw.to_string(),
        }]
    }
}

/// Extract CQ codes from a string.
fn parse_cq_codes(raw: &str) -> Vec<MessageSegment> {
    let mut segments = Vec::new();
    let mut remaining = raw;

    while let Some(start) = remaining.find("[CQ:") {
        // Text before the CQ code
        if start > 0 {
            let text = &remaining[..start];
            if !text.trim().is_empty() {
                segments.push(MessageSegment::Text {
                    content: text.to_string(),
                });
            }
        }

        let cq_end = remaining[start..].find(']').map(|i| start + i + 1);
        if let Some(end) = cq_end {
            let cq_content = &remaining[start + 4..end - 1]; // strip [CQ: and ]
            let parts = cq_content.split(',');
            let mut cq_type = "";
            let mut params = Vec::new();
            for (i, part) in parts.enumerate() {
                if i == 0 {
                    cq_type = part;
                } else {
                    params.push(part);
                }
            }

            match cq_type {
                "at" => {
                    let qq = params
                        .iter()
                        .find(|p| p.starts_with("qq="))
                        .map(|p| p[3..].to_string())
                        .unwrap_or_default();
                    segments.push(MessageSegment::At { qq });
                }
                "face" => {
                    let id = params
                        .iter()
                        .find(|p| p.starts_with("id="))
                        .and_then(|p| p[3..].parse::<i32>().ok())
                        .unwrap_or(0);
                    let text = params
                        .iter()
                        .find(|p| p.starts_with("text="))
                        .map(|p| p[5..].to_string());
                    segments.push(MessageSegment::Face { id, text });
                }
                "image" => {
                    let file = params
                        .iter()
                        .find(|p| p.starts_with("file="))
                        .map(|p| p[5..].to_string())
                        .unwrap_or_default();
                    let url = params
                        .iter()
                        .find(|p| p.starts_with("url="))
                        .map(|p| p[4..].to_string());
                    segments.push(MessageSegment::Image { file, url });
                }
                "reply" => {
                    let id = params
                        .iter()
                        .find(|p| p.starts_with("id="))
                        .map(|p| p[3..].to_string())
                        .unwrap_or_default();
                    segments.push(MessageSegment::Reply { id });
                }
                "record" => {
                    let file = params
                        .iter()
                        .find(|p| p.starts_with("file="))
                        .map(|p| p[5..].to_string())
                        .unwrap_or_default();
                    segments.push(MessageSegment::Record { file });
                }
                _ => {
                    segments.push(MessageSegment::Unknown {
                        raw: cq_content.to_string(),
                    });
                }
            }

            remaining = &remaining[end..];
        } else {
            // Malformed CQ code, treat as text
            segments.push(MessageSegment::Text {
                content: remaining.to_string(),
            });
            remaining = "";
        }
    }

    // Remaining text
    if !remaining.is_empty() && !remaining.trim().is_empty() {
        segments.push(MessageSegment::Text {
            content: remaining.to_string(),
        });
    }

    segments
}

/// 将 CQ 原始文本转换为协议无关的纯文本，并标记是否提及当前机器人。
pub fn normalize_text(raw: &str, self_qq_id: i64) -> (String, bool) {
    let mut normalized = raw.to_string();
    let mut at_bot = false;

    // Check for @bot
    let at_pattern = format!("[CQ:at,qq={}]", self_qq_id);
    if normalized.contains(&at_pattern) {
        at_bot = true;
        normalized = normalized.replace(&at_pattern, "");
    }

    // Replace other @ mentions with @user
    // Match [CQ:at,qq=xxxxx]
    let re = regex::Regex::new(r"\[CQ:at,qq=(\d+)\]").unwrap();
    normalized = re.replace_all(&normalized, "@user").to_string();

    // Remove other CQ codes (image, face, record, etc.)
    let re_cq = regex::Regex::new(r"\[CQ:[^\]]+\]").unwrap();
    normalized = re_cq.replace_all(&normalized, "").to_string();

    // Clean up extra whitespace (collapse multiple spaces)
    let re_spaces = regex::Regex::new(r"  +").unwrap();
    normalized = re_spaces.replace_all(&normalized, " ").to_string();
    normalized = normalized.trim().to_string();

    (normalized, at_bot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_only() {
        let segments = parse_message_segments("你好");
        assert_eq!(segments.len(), 1);
        assert!(matches!(segments[0], MessageSegment::Text { ref content } if content == "你好"));
    }

    #[test]
    fn test_parse_at() {
        let segments = parse_message_segments("[CQ:at,qq=123456] 今天天气怎么样");
        assert_eq!(segments.len(), 2);
        assert!(matches!(segments[0], MessageSegment::At { ref qq } if qq == "123456"));
        assert!(
            matches!(segments[1], MessageSegment::Text { ref content } if content.trim() == "今天天气怎么样")
        );
    }

    #[test]
    fn test_parse_image() {
        let segments = parse_message_segments(
            "看这个 [CQ:image,file=test.jpg,url=https://example.com/test.jpg]",
        );
        assert_eq!(segments.len(), 2);
        assert!(
            matches!(segments[1], MessageSegment::Image { ref file, .. } if file == "test.jpg")
        );
    }

    #[test]
    fn test_normalize_text_strips_cq() {
        let (text, at_bot) = normalize_text("hello [CQ:face,id=123]", 10001);
        assert_eq!(text, "hello");
        assert!(!at_bot);
    }

    #[test]
    fn test_normalize_text_detects_at_bot() {
        let (text, at_bot) = normalize_text("[CQ:at,qq=10001] 你好", 10001);
        assert!(at_bot);
        assert_eq!(text, "你好");
    }

    #[test]
    fn test_normalize_text_replaces_other_at() {
        let (text, at_bot) = normalize_text("[CQ:at,qq=99999] 你好", 10001);
        assert!(!at_bot); // not the bot
        assert_eq!(text, "@user 你好");
    }

    #[test]
    fn test_normalize_text_mixed() {
        let raw = "[CQ:at,qq=10001] 看看这个 [CQ:image,file=cat.jpg] 可爱吗？";
        let (text, at_bot) = normalize_text(raw, 10001);
        assert!(at_bot);
        assert_eq!(text, "看看这个 可爱吗？");
    }
}
