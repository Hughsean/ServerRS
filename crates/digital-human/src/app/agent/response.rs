pub(crate) const FALLBACK_REPLY: &str =
    "抱歉，我刚才处理这条消息时遇到了一点问题。你可以换个说法再发一次，我会继续帮你。";

pub(crate) fn fallback_reply() -> String {
    FALLBACK_REPLY.to_string()
}

/// 移除推理模型混入 `content` 的思考过程。
///
/// 部分 OpenAI-compatible 服务不会把 reasoning 单独放在字段中，而是返回
/// `<think>...</think>最终回答`；也有模型只返回 `</think>` 结束标签。最终回复在
/// 持久化、TTS 和 API 返回前必须统一清理这些内容。
fn strip_reasoning_artifacts(content: &str) -> &str {
    const OPENING_TAG: &str = "<think>";
    const CLOSING_TAG: &str = "</think>";

    let content = content.trim();
    if let Some(closing_index) = rfind_ascii_case_insensitive(content, CLOSING_TAG) {
        return content[closing_index + CLOSING_TAG.len()..].trim();
    }

    if let Some(opening_index) = find_ascii_case_insensitive(content, OPENING_TAG) {
        // 未闭合的思考块不能安全地区分思考和最终答案；只保留标签前已经完成的内容。
        return content[..opening_index].trim();
    }

    content
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn rfind_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .rposition(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

/// 移除某些模型在最终答案前回显的序列化工具调用。
fn strip_leading_tool_call_artifacts(content: &str) -> &str {
    const CLOSING_TAG: &str = "</tool_call>";
    const OPENING_MARKERS: [&str; 3] = ["<tool_call>", "<|tool_call|>", "_icall_"];

    let mut remaining = content.trim();
    loop {
        let Some(closing_index) = remaining.find(CLOSING_TAG) else {
            break;
        };
        let artifact = &remaining[..closing_index];
        let starts_with_marker = OPENING_MARKERS
            .iter()
            .any(|marker| artifact.trim_start().starts_with(marker));
        let looks_like_tool_call =
            artifact.contains("\"name\"") && artifact.contains("\"arguments\"");

        if !starts_with_marker || !looks_like_tool_call {
            break;
        }

        remaining = remaining[closing_index + CLOSING_TAG.len()..].trim_start();
    }

    remaining
}

/// 确保最终内容干净且非空；必要时返回兼容的中文回退文本。
pub(crate) fn normalize_final_content(content: String) -> String {
    let reasoning_artifact_detected = find_ascii_case_insensitive(&content, "<think>").is_some()
        || find_ascii_case_insensitive(&content, "</think>").is_some();
    let content = strip_reasoning_artifacts(&content);
    let content = strip_leading_tool_call_artifacts(content);
    if reasoning_artifact_detected {
        tracing::warn!(
            cleaned_length = content.chars().count(),
            "模型回复包含思考内容，已在持久化和返回前清理"
        );
    }
    if content.is_empty() {
        fallback_reply()
    } else {
        content.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_content_is_stable() {
        assert_eq!(
            fallback_reply(),
            "抱歉，我刚才处理这条消息时遇到了一点问题。你可以换个说法再发一次，我会继续帮你。"
        );
    }

    #[test]
    fn normalization_strips_leading_serialized_tool_calls() {
        let content = concat!(
            "<tool_call>{\"name\":\"weather\",\"arguments\":{}}</tool_call>",
            "合肥今天多云。"
        );

        assert_eq!(normalize_final_content(content.into()), "合肥今天多云。");
    }

    #[test]
    fn normalization_strips_paired_reasoning_block() {
        let content = "<think>需要先分析用户意图。</think>嗨，今天过得怎么样？";

        assert_eq!(
            normalize_final_content(content.into()),
            "嗨，今天过得怎么样？"
        );
    }

    #[test]
    fn normalization_strips_reasoning_before_orphaned_closing_tag() {
        let content = "好的，现在用户说您好，我需要保持自然对话。\n</think>嗨！最近怎么样？";

        assert_eq!(normalize_final_content(content.into()), "嗨！最近怎么样？");
    }

    #[test]
    fn normalization_uses_fallback_for_unclosed_reasoning_block() {
        let content = "<THINK>需要分析用户意图，但模型没有输出结束标签。";

        assert_eq!(normalize_final_content(content.into()), fallback_reply());
    }

    #[test]
    fn normalization_uses_fallback_for_empty_content() {
        assert_eq!(normalize_final_content("   ".into()), fallback_reply());
    }

    #[test]
    fn normalization_preserves_non_artifact_content() {
        let content = "示例里有 </tool_call>，但不是工具调用。".to_string();
        assert_eq!(normalize_final_content(content.clone()), content);
    }
}
