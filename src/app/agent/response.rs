pub(crate) const FALLBACK_REPLY: &str =
    "抱歉，我刚才处理这条消息时遇到了一点问题。你可以换个说法再发一次，我会继续帮你。";

pub(crate) fn fallback_reply() -> String {
    FALLBACK_REPLY.to_string()
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
    let content = strip_leading_tool_call_artifacts(&content);
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
    fn normalization_uses_fallback_for_empty_content() {
        assert_eq!(normalize_final_content("   ".into()), fallback_reply());
    }

    #[test]
    fn normalization_preserves_non_artifact_content() {
        let content = "示例里有 </tool_call>，但不是工具调用。".to_string();
        assert_eq!(normalize_final_content(content.clone()), content);
    }
}
