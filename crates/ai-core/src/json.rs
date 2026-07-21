use serde::de::DeserializeOwned;

pub fn parse_llm_json<T>(raw: &str) -> Result<T, serde_json::Error>
where
    T: DeserializeOwned,
{
    let cleaned = clean_llm_json_response(raw);
    serde_json::from_str(&cleaned)
}

pub fn clean_llm_json_response(raw: &str) -> String {
    let without_thinking = strip_thinking_blocks(raw);
    let trimmed = without_thinking.trim();
    extract_first_json_value(trimmed)
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

fn strip_thinking_blocks(input: &str) -> String {
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

fn extract_first_json_value(input: &str) -> Option<&str> {
    let object_start = input.find('{');
    let array_start = input.find('[');
    let start = match (object_start, array_start) {
        (Some(object), Some(array)) => object.min(array),
        (Some(object), None) => object,
        (None, Some(array)) => array,
        (None, None) => return None,
    };

    let mut expected_closers = Vec::new();
    let mut in_string = false;
    let mut escaped = false;

    for (offset, character) in input[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '{' => expected_closers.push('}'),
            '[' => expected_closers.push(']'),
            '}' | ']' => {
                if expected_closers.pop() != Some(character) {
                    return None;
                }
                if expected_closers.is_empty() {
                    let end = start + offset + character.len_utf8();
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
    use super::*;

    #[test]
    fn ignores_reasoning_and_extracts_first_complete_value() {
        let cleaned = clean_llm_json_response(
            r#"<think>{"draft":true}</think> prefix {"answer":[1,2]} suffix"#,
        );

        assert_eq!(cleaned, r#"{"answer":[1,2]}"#);
    }
}
