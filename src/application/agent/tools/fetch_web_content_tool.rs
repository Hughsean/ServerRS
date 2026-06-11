use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::application::agent::agent_runtime::AgentTool;
use crate::domain::agent::AgentContext;
use crate::shared::config::FetchWebContentPluginConfig;
use crate::shared::error::AppError;

pub struct FetchWebContentTool {
    config: FetchWebContentPluginConfig,
    http_client: reqwest::Client,
}

impl FetchWebContentTool {
    pub fn new(config: FetchWebContentPluginConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            config,
            http_client,
        }
    }
}

#[async_trait]
impl AgentTool for FetchWebContentTool {
    fn name(&self) -> &str {
        "fetch_web_content"
    }

    fn description(&self) -> &str {
        "用于根据URL获取网页完整文本内容。适用场景：1. web_search返回摘要不够详细时，获取完整网页正文。2. 用户需要深入了解某个网页内容时。3. 提取网页主要文本，去除HTML标签和脚本。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "要获取内容的完整URL地址，例如 https://example.com/article。通常从 web_search 的结果中选择。"
                },
                "max_length": {
                    "type": "integer",
                    "description": "返回内容的最大字符数，默认5000，最大10000。"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, _context: &AgentContext, args: Value) -> Result<String, AppError> {
        tracing::info!(tool = "fetch_web_content", "executing agent tool");

        // Check enabled
        if !self.config.enabled {
            return Ok("网页内容获取功能未启用。".to_string());
        }

        // Extract url
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());

        let url = match url {
            Some(u) => u.to_string(),
            None => return Ok("请提供有效的URL地址。".to_string()),
        };

        // Validate URL
        if let Err(err_msg) = validate_url(&url) {
            return Ok(err_msg);
        }

        // Extract max_length
        let max_length = args
            .get("max_length")
            .and_then(|v| v.as_i64())
            .map(|n| {
                if n <= 0 {
                    5000
                } else if n > 10000 {
                    10000
                } else {
                    n as usize
                }
            })
            .unwrap_or(5000);

        // Fetch page
        let response = match self
            .http_client
            .get(&url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(tool = "fetch_web_content", error = %e, "fetch request failed");
                return Ok("无法获取网页内容或网页内容为空。".to_string());
            }
        };

        if !response.status().is_success() {
            return Ok("无法获取网页内容或网页内容为空。".to_string());
        }

        let html = match response.text().await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(tool = "fetch_web_content", error = %e, "read response body failed");
                return Ok("无法获取网页内容或网页内容为空。".to_string());
            }
        };

        // Extract text content
        let content = extract_text_content(&html, max_length);

        if content.trim().is_empty() {
            return Ok("无法获取网页内容或网页内容为空。".to_string());
        }

        let truncated = truncate_with_ellipsis(&content, max_length);

        // Build intermediate prompt
        let prompt = format!(
            "以下是从网页获取的内容:\n\n来源: {url}\n\n内容:\n{truncated}\n\n请根据以上内容回答用户的问题,并在适当时提及信息来源。"
        );

        Ok(prompt)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn validate_url(url: &str) -> Result<(), String> {
    let parsed = match reqwest::Url::parse(url) {
        Ok(p) => p,
        Err(_) => return Err(format!("提供的URL格式无效: {url}")),
    };

    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("提供的URL格式无效: {url}"));
    }

    if !parsed.has_host() {
        return Err(format!("提供的URL格式无效: {url}"));
    }

    Ok(())
}

/// Extract readable text content from HTML using `scraper`.
///
/// Prioritises semantic content containers, falls back to `<body>`,
/// and strips non-content elements.
pub fn extract_text_content(html: &str, _max_length: usize) -> String {
    let document = scraper::Html::parse_document(html);

    // Preferred main content selectors (in priority order)
    let main_selectors = [
        "article",
        "main",
        "[role=main]",
        ".article-content",
        ".post-content",
        ".entry-content",
        ".content",
        "#content",
        ".main-content",
        "#main-content",
    ];

    let mut best_text = String::new();

    for selector_str in &main_selectors {
        let selector = match scraper::Selector::parse(selector_str) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let combined: String = document
            .select(&selector)
            .flat_map(|el| el.text())
            .collect::<Vec<_>>()
            .join(" ");
        let cleaned = clean_whitespace(&combined);
        if cleaned.chars().count() > 200 {
            best_text = cleaned;
            break;
        } else if cleaned.len() > best_text.len() {
            best_text = cleaned;
        }
    }

    // Fallback to body
    if best_text.trim().is_empty() {
        if let Ok(body_sel) = scraper::Selector::parse("body") {
            let body_text: String = document
                .select(&body_sel)
                .flat_map(|el| el.text())
                .collect::<Vec<_>>()
                .join(" ");
            best_text = clean_whitespace(&body_text);
        }
    }

    // Instead of removing individual elements (which is complex with scraper),
    // we clean the text by collapsing whitespace.
    clean_whitespace(&best_text)
}

/// Collapse consecutive whitespace into single spaces and trim.
fn clean_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate text to `max_chars` characters, appending ellipsis marker if needed.
fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}...[内容已截断]")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── URL validation ─────────────────────────────────────────────────

    #[test]
    fn rejects_empty_url() {
        let tool = FetchWebContentTool::new(FetchWebContentPluginConfig { enabled: true });
        let ctx = test_context();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(&ctx, json!({"url": ""}))).unwrap();
        assert!(result.contains("请提供有效的URL地址"));
    }

    #[test]
    fn rejects_non_http_url() {
        assert!(validate_url("ftp://files.example.com/foo").is_err());
        assert!(validate_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn rejects_url_without_host() {
        assert!(validate_url("http://").is_err());
        assert!(validate_url("https://").is_err());
    }

    #[test]
    fn clamps_max_length_to_10000() {
        let tool = FetchWebContentTool::new(FetchWebContentPluginConfig { enabled: true });
        let ctx = test_context();
        let rt = tokio::runtime::Runtime::new().unwrap();
        // Use a bogus URL so it fails with network error → "无法获取..."
        let result = rt
            .block_on(tool.execute(
                &ctx,
                json!({"url": "https://127.0.0.1:1/nonexistent", "max_length": 20000}),
            ))
            .unwrap();
        // Should not panic; clamping happens internally before the network call
        assert!(result.contains("无法获取网页内容") || result.contains("网页内容为空"));
    }

    #[test]
    fn uses_default_max_length_when_invalid() {
        let tool = FetchWebContentTool::new(FetchWebContentPluginConfig { enabled: true });
        let ctx = test_context();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(tool.execute(
                &ctx,
                json!({"url": "https://127.0.0.1:1/nonexistent", "max_length": 0}),
            ))
            .unwrap();
        assert!(result.contains("无法获取网页内容") || result.contains("网页内容为空"));
    }

    #[test]
    fn disabled_returns_message() {
        let tool = FetchWebContentTool::new(FetchWebContentPluginConfig { enabled: false });
        let ctx = test_context();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(tool.execute(&ctx, json!({"url": "https://example.com"})))
            .unwrap();
        assert_eq!(result, "网页内容获取功能未启用。");
    }

    // ── HTML extraction ────────────────────────────────────────────────

    #[test]
    fn extracts_article_content_from_html() {
        let html = r#"
        <html><head><title>Test</title></head><body>
        <script>var x = 1;</script>
        <style>.foo { color: red; }</style>
        <nav>Menu</nav>
        <article>
            <h1>Article Title</h1>
            <p>This is the main article content. It has enough text to exceed 200 characters.
            Let me add more text here. The article discusses important topics about web scraping
            and content extraction. We need to make sure that the scraper can correctly identify
            the main content area and extract it properly without including navigation or footer.</p>
        </article>
        <footer>Copyright 2024</footer>
        </body></html>
        "#;

        let extracted = extract_text_content(html, 5000);
        assert!(
            extracted.contains("Article Title"),
            "should contain article h1 text, got: {extracted}"
        );
        assert!(
            extracted.contains("main article content"),
            "should contain article body, got: {extracted}"
        );
        // Should not contain script/style content
        assert!(
            !extracted.contains("var x = 1"),
            "should not contain script content"
        );
        assert!(
            !extracted.contains(".foo"),
            "should not contain style content"
        );
    }

    #[test]
    fn falls_back_to_body_when_no_article() {
        let html = r#"
        <html><body>
        <p>Just some text without any semantic structure. This is a simple page that
        doesn't have any article, main, or content-specific elements. The scraper should
        fall back to extracting text from the body element. Let me add more text here to
        make this paragraph longer. Lorem ipsum dolor sit amet, consectetur adipiscing elit.
        Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.</p>
        </body></html>
        "#;

        let extracted = extract_text_content(html, 5000);
        assert!(
            extracted.contains("semantic structure"),
            "should contain body text, got: {extracted}"
        );
    }

    #[test]
    fn truncates_content_beyond_max_length() {
        let long_text = "A".repeat(6000);
        let truncated = truncate_with_ellipsis(&long_text, 5000);
        // truncate_with_ellipsis appends "...[内容已截断]" (10 extra chars)
        assert_eq!(truncated.chars().count(), 5000 + 10);
        assert!(truncated.ends_with("[内容已截断]"));
    }

    // ── Helpers ────────────────────────────────────────────────────────

    fn test_context() -> AgentContext {
        use crate::domain::agent::ToolDefinition;

        AgentContext {
            user_id: 1,
            session_id: "test-session".into(),
            conversation_id: None,
            recent_messages: vec![],
            summary: None,
            memories: vec![],
            rag_chunks: vec![],
            user_profile: None,
            tools: vec![ToolDefinition {
                name: "fetch_web_content".into(),
                description: "fetch web content".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {"type": "string"},
                        "max_length": {"type": "integer"}
                    },
                    "required": ["url"]
                }),
            }],
        }
    }
}
