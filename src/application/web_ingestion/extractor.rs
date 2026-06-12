//! HTML → readable text extractor.
//!
//! Removes script/style/nav/footer/header/ad noise, preserves title,
//! paragraphs, lists, tables. Normalises whitespace.

/// Extract clean, readable text from an HTML document.
///
/// Returns `(title, clean_text)` where `title` may be empty.
pub fn extract_clean_text(html: &str) -> (String, String) {
    let document = scraper::Html::parse_document(html);

    // Extract <title>
    let title = extract_title(&document);

    // Remove unwanted elements
    let selector = scraper::Selector::parse(
        "script, style, nav, footer, header, iframe, noscript, \
         [role=\"navigation\"], [role=\"banner\"], [role=\"contentinfo\"], \
         .ad, .ads, .advertisement, .sidebar, .nav, .footer, .header, .menu",
    )
    .unwrap();

    let body_selector = scraper::Selector::parse("body").unwrap();
    let clean_text = if let Some(body) = document.select(&body_selector).next() {
        collect_text(body, &selector)
    } else {
        let root = document.root_element();
        collect_text(root, &selector)
    };

    // Normalise whitespace
    let clean_text = normalize_whitespace(&clean_text);

    (title, clean_text)
}

fn extract_title(document: &scraper::Html) -> String {
    let sel = scraper::Selector::parse("title").unwrap();
    document
        .select(&sel)
        .next()
        .map(|e| e.text().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn collect_text(root: scraper::ElementRef<'_>, exclude: &scraper::Selector) -> String {
    let mut text = String::new();
    collect_text_recursive(root, exclude, &mut text);
    text
}

fn collect_text_recursive(
    node: scraper::ElementRef<'_>,
    exclude: &scraper::Selector,
    buf: &mut String,
) {
    // Skip excluded elements
    if node.select(exclude).next().is_some() {
        // The node itself matches an exclusion selector or contains excluded elements?
        // scraper's select works on descendants, so we check if the node itself matches.
    }

    // Check if this element itself matches the exclusion
    let tag = node.value().name();
    if EXCLUDE_TAGS.contains(&tag) {
        return;
    }

    for child in node.children() {
        match child.value() {
            scraper::Node::Text(t) => {
                let s = t.text.trim();
                if !s.is_empty() {
                    if !buf.is_empty() && !buf.ends_with(' ') {
                        buf.push(' ');
                    }
                    buf.push_str(s);
                }
            }
            scraper::Node::Element(e) => {
                let tag = e.name();
                if EXCLUDE_TAGS.contains(&tag) {
                    continue;
                }
                // Block-level elements add line breaks
                if BLOCK_TAGS.contains(&tag) {
                    buf.push('\n');
                }
                // Recurse into children
                if let Some(el_ref) = scraper::ElementRef::wrap(child) {
                    collect_text_recursive(el_ref, exclude, buf);
                }
            }
            _ => {}
        }
    }
    // Add newline after block-level elements
    if BLOCK_TAGS.contains(&node.value().name()) {
        buf.push('\n');
    }
}

const EXCLUDE_TAGS: &[&str] = &[
    "script",
    "style",
    "nav",
    "footer",
    "header",
    "iframe",
    "noscript",
    "aside",
    "figure",
    "figcaption",
];

const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "li",
    "tr",
    "section",
    "article",
    "blockquote",
    "pre",
    "br",
    "hr",
    "table",
    "ul",
    "ol",
];

fn normalize_whitespace(text: &str) -> String {
    // Collapse multiple spaces/tabs into single space
    let mut result = String::with_capacity(text.len());
    let mut prev_was_space = false;
    let mut prev_was_newline = false;

    for ch in text.chars() {
        if ch == '\n' || ch == '\r' {
            if !prev_was_newline {
                result.push('\n');
                prev_was_newline = true;
            }
            prev_was_space = false;
        } else if ch.is_whitespace() {
            if !prev_was_space && !prev_was_newline {
                result.push(' ');
                prev_was_space = true;
            }
        } else {
            result.push(ch);
            prev_was_space = false;
            prev_was_newline = false;
        }
    }
    result.trim().to_string()
}

/// Check if clean text is too short to be useful.
pub fn is_too_short(text: &str, min_chars: usize) -> bool {
    text.chars().count() < min_chars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_removes_script() {
        let html = "<html><head><title>Test</title></head><body><p>Hello</p><script>alert('xss')</script><p>World</p></body></html>";
        let (title, text) = extract_clean_text(html);
        assert_eq!(title, "Test");
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("alert"));
        assert!(!text.contains("xss"));
    }

    #[test]
    fn test_extract_empty() {
        let (title, text) = extract_clean_text("");
        assert_eq!(title, "");
        assert_eq!(text, "");
    }

    #[test]
    fn test_is_too_short() {
        assert!(is_too_short("ab", 10));
        assert!(!is_too_short("this is long enough text", 10));
    }

    #[test]
    fn test_normalize_whitespace() {
        let input = "hello   world\n\n\nfoo  bar";
        let output = normalize_whitespace(input);
        assert!(output.contains("hello world"));
        assert!(output.contains("foo bar"));
    }
}
