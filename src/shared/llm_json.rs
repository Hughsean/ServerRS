use serde::de::DeserializeOwned;

/// Parse a JSON value from an LLM response after removing common non-JSON
/// wrappers such as Qwen think blocks and markdown fences.
pub fn parse_llm_json<T>(raw: &str) -> Result<T, serde_json::Error>
where
    T: DeserializeOwned,
{
    let cleaned = clean_llm_json_response(raw);
    serde_json::from_str(&cleaned)
}

/// Return the first complete JSON object or array from an LLM response.
///
/// The cleanup intentionally removes `<think>...</think>` before scanning, so
/// JSON-shaped drafts inside Qwen reasoning blocks do not get parsed as output.
/// Some Qwen/Ollama responses only include a dangling `</think>` marker; in
/// that case everything up to and including the marker is treated as reasoning.
pub fn clean_llm_json_response(raw: &str) -> String {
    let without_thinking = strip_thinking_blocks(raw);
    let trimmed = without_thinking.trim();
    extract_first_json_value(trimmed)
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

pub fn strip_thinking_blocks(input: &str) -> String {
    let mut rest = input;
    let mut output = String::with_capacity(input.len());

    loop {
        let Some(start) = rest.find("<think>") else {
            if output.is_empty() {
                if let Some(end) = rest.find("</think>") {
                    rest = &rest[end + "</think>".len()..];
                    output.push_str(rest);
                    break;
                }
            }
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..start]);

        let after_start = &rest[start + "<think>".len()..];
        let Some(end) = after_start.find("</think>") else {
            break;
        };
        rest = &after_start[end + "</think>".len()..];
    }

    output
}

pub fn extract_first_json_value(input: &str) -> Option<&str> {
    let object_start = input.find('{');
    let array_start = input.find('[');
    let start = match (object_start, array_start) {
        (Some(o), Some(a)) => o.min(a),
        (Some(o), None) => o,
        (None, Some(a)) => a,
        (None, None) => return None,
    };

    let mut expected_closers = Vec::new();
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in input[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => expected_closers.push('}'),
            '[' => expected_closers.push(']'),
            '}' | ']' => {
                if expected_closers.pop() != Some(ch) {
                    return None;
                }
                if expected_closers.is_empty() {
                    let end = start + offset + ch.len_utf8();
                    return Some(&input[start..end]);
                }
            }
            _ => {}
        }
    }

    None
}

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
