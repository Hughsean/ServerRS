pub use ai_core::{clean_llm_json_response, parse_llm_json};

/// Parse a JSON value from an LLM response after removing common non-JSON
/// wrappers such as Qwen think blocks and markdown fences.

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Decision {
        decision: String,
        candidate_memory_id: Option<u64>,
    }

    #[test]
    fn extracts_object_after_qwen_think_block() {
        let cleaned = clean_llm_json_response(
            r#"<think>{"draft":true}</think>
            prefix
            {"decision":"new","candidate_memory_id":null}
            suffix"#,
        );

        assert_eq!(cleaned, r#"{"decision":"new","candidate_memory_id":null}"#);
    }

    #[test]
    fn drops_dangling_qwen_think_close_prefix() {
        let cleaned = clean_llm_json_response(
            r#"（内心OS：这里可能有 {"draft": true} 这种草稿。）
            </think>
            [{"memory_type":"fact","content":"用户叫 Alice"}]"#,
        );

        assert_eq!(
            cleaned,
            r#"[{"memory_type":"fact","content":"用户叫 Alice"}]"#
        );
    }

    #[test]
    fn extracts_array_from_markdown_fence() {
        let cleaned = clean_llm_json_response(
            r#"```json
            [{"index":0,"decision":"new"}]
            ```"#,
        );

        assert_eq!(cleaned, r#"[{"index":0,"decision":"new"}]"#);
    }

    #[test]
    fn handles_nested_json_and_brackets_inside_strings() {
        let cleaned = clean_llm_json_response(
            r#"noise {"outer":{"text":"literal } and ] chars"},"items":[1,2]} trailing"#,
        );

        assert_eq!(
            cleaned,
            r#"{"outer":{"text":"literal } and ] chars"},"items":[1,2]}"#
        );
    }

    #[test]
    fn parses_cleaned_json() {
        let parsed: Decision = parse_llm_json(
            r#"<think>{"decision":"new"}</think>
            {"decision":"new_evidence","candidate_memory_id":7}"#,
        )
        .unwrap();

        assert_eq!(
            parsed,
            Decision {
                decision: "new_evidence".into(),
                candidate_memory_id: Some(7)
            }
        );
    }
}
