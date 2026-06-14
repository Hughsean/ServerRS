use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::app::agent::agent_runtime::AgentTool;
use crate::domain::agent::AgentContext;
use crate::shared::config::BaiduBaikePluginConfig;
use crate::shared::error::AppError;

pub struct BaiduBaikeTool {
    config: BaiduBaikePluginConfig,
    http_client: reqwest::Client,
}

impl BaiduBaikeTool {
    pub fn new(config: BaiduBaikePluginConfig) -> Self {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(5))
            .no_proxy();
        if !config.proxy_url.trim().is_empty() {
            builder = builder.proxy(
                reqwest::Proxy::all(config.proxy_url.trim())
                    .expect("invalid baidu_baike proxy URL"),
            );
        }
        let http_client = builder.build().unwrap_or_else(|_| reqwest::Client::new());

        Self {
            config,
            http_client,
        }
    }
}

#[async_trait]
impl AgentTool for BaiduBaikeTool {
    fn name(&self) -> &str {
        "get_baidu_baike"
    }

    fn description(&self) -> &str {
        "用于查询相对稳定的百科类实体介绍，例如人物生平、概念解释、历史事件、机构简介。它不是实时信息源，不适合查询「现任/最新/今天/当前状态/价格/法规/新闻」等可能变化的信息。若用户问题涉及实时性，只能说明百科信息可能滞后，不得声称已验证最新情况。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keyword": {
                    "type": "string",
                    "description": "要查询的核心名词或实体名称，例如「日本首相」「量子力学」「长城」。直接使用用户关心的实体名称即可。"
                }
            },
            "required": ["keyword"]
        })
    }

    async fn execute(&self, _context: &AgentContext, args: Value) -> Result<String, AppError> {
        tracing::info!(tool = "get_baidu_baike", "executing agent tool");

        // Check enabled
        if !self.config.enabled {
            return Ok("百度百科插件未启用。".to_string());
        }

        // Extract keyword
        let keyword = args
            .get("keyword")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());

        let keyword = match keyword {
            Some(k) => k.to_string(),
            None => return Ok("请提供关键词。".to_string()),
        };

        // Build URL
        let encoded = urlencoding::encode(&keyword);
        let url = format!("https://wapbaike.baidu.com/item/{encoded}");

        // Fetch page
        let response = match self
            .http_client
            .get(&url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Linux; Android 13) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Mobile Safari/537.36",
            )
            .header("Accept", "text/html,application/xhtml+xml")
            .header("Accept-Language", "zh-CN,zh;q=0.9")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(tool = "get_baidu_baike", error = %e, "baike request failed");
                return Ok(format!("查询百度百科时发生错误: {e}"));
            }
        };

        let status = response.status();

        if status.as_u16() == 404 {
            return Ok(format!("未找到关于「{keyword}」的百度百科内容。"));
        }

        if !status.is_success() {
            return Ok(format!("查询百度百科时发生错误: HTTP {status}"));
        }

        let html = match response.text().await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(tool = "get_baidu_baike", error = %e, "read baike body failed");
                return Ok(format!("查询百度百科时发生错误: {e}"));
            }
        };

        // Parse HTML
        let parsed = parse_baike_html(&html, &keyword, &url);

        if parsed.trim().is_empty() {
            return Ok(format!("未找到关于「{keyword}」的百度百科内容。"));
        }

        Ok(parsed)
    }
}

// ── HTML parsing helpers ──────────────────────────────────────────────────────

/// Parse Baidu Baike HTML and extract structured info.
pub fn parse_baike_html(html: &str, fallback_title: &str, url: &str) -> String {
    let document = scraper::Html::parse_document(html);

    // 1. Title
    let title = extract_title(&document, fallback_title);

    // 2. Summary
    let summary = extract_summary(&document);

    // 3. Basic info
    let basic_info = extract_basic_info(&document);

    // 4. If both summary and basic_info are empty, check polysemy
    if summary.is_empty() && basic_info.is_empty() {
        if let Some(polysemy) = extract_polysemy(&document) {
            return polysemy;
        }
    }

    // 5. Format result
    let mut result = format!("百度百科: {title}\n链接: {url}\n");
    if !summary.is_empty() {
        result.push_str(&format!("\n摘要:\n{summary}\n"));
    }
    if !basic_info.is_empty() {
        result.push_str(&format!("\n基本信息:\n{basic_info}"));
    }

    result.trim().to_string()
}

fn extract_title(document: &scraper::Html, fallback: &str) -> String {
    if let Ok(sel) = scraper::Selector::parse("h1") {
        if let Some(el) = document.select(&sel).next() {
            let title: String = el.text().collect::<Vec<_>>().join("").trim().to_string();
            if !title.is_empty() {
                return title;
            }
        }
    }
    fallback.to_string()
}

fn extract_summary(document: &scraper::Html) -> String {
    // Try .lemma-summary first
    if let Ok(sel) = scraper::Selector::parse(".lemma-summary") {
        if let Some(el) = document.select(&sel).next() {
            let text: String = el.text().collect::<Vec<_>>().join(" ");
            let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if !cleaned.is_empty() {
                return cleaned;
            }
        }
    }

    // Fallback to meta description
    if let Ok(sel) = scraper::Selector::parse("meta[name=description]") {
        if let Some(el) = document.select(&sel).next() {
            if let Some(content) = el.value().attr("content") {
                let cleaned = content.split_whitespace().collect::<Vec<_>>().join(" ");
                if !cleaned.is_empty() {
                    return cleaned;
                }
            }
        }
    }

    String::new()
}

fn extract_basic_info(document: &scraper::Html) -> String {
    let name_sel = match scraper::Selector::parse(".basicInfo-item.name") {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    let value_sel = match scraper::Selector::parse(".basicInfo-item.value") {
        Ok(s) => s,
        Err(_) => return String::new(),
    };

    let names: Vec<String> = document
        .select(&name_sel)
        .map(|el| {
            el.text()
                .collect::<Vec<_>>()
                .join("")
                .replace('\u{00A0}', "")
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();

    let values: Vec<String> = document
        .select(&value_sel)
        .map(|el| el.text().collect::<Vec<_>>().join("").trim().to_string())
        .collect();

    if names.is_empty() || values.is_empty() {
        return String::new();
    }

    let max_pairs = names.len().min(values.len());
    names
        .iter()
        .zip(values.iter())
        .take(max_pairs)
        .map(|(name, value)| format!("{name}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_polysemy(document: &scraper::Html) -> Option<String> {
    let sel = scraper::Selector::parse(".polysemy-list li a").ok()?;
    let items: Vec<String> = document
        .select(&sel)
        .filter_map(|el| {
            let text: String = el.text().collect::<Vec<_>>().join("").trim().to_string();
            if text.is_empty() {
                None
            } else {
                Some(format!("- {text}"))
            }
        })
        .collect();

    if items.is_empty() {
        return None;
    }

    Some(format!(
        "这是一个多义词，请更具体地描述，可能的含义包括：\n{}",
        items.join("\n")
    ))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_keyword_returns_message() {
        let tool = BaiduBaikeTool::new(BaiduBaikePluginConfig {
            enabled: true,
            ..Default::default()
        });
        let ctx = test_context();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(tool.execute(&ctx, json!({"keyword": ""})))
            .unwrap();
        assert!(result.contains("请提供关键词"));
    }

    #[test]
    fn disabled_returns_message() {
        let tool = BaiduBaikeTool::new(BaiduBaikePluginConfig {
            enabled: false,
            ..Default::default()
        });
        let ctx = test_context();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(tool.execute(&ctx, json!({"keyword": "日本首相"})))
            .unwrap();
        assert_eq!(result, "百度百科插件未启用。");
    }

    #[test]
    fn extracts_title_summary_and_basic_info() {
        let html = r#"
        <html>
        <head><meta name="description" content="日本首相是日本的政府首脑。"></head>
        <body>
            <h1>日本首相</h1>
            <div class="lemma-summary">
                日本内阁总理大臣（日语：内閣総理大臣，英语：Prime Minister of Japan），
                简称日本首相、总理，是日本国的政府首脑。
            </div>
            <div class="basicInfo">
                <span class="basicInfo-item name">中文名&nbsp;</span>
                <span class="basicInfo-item value">日本内阁总理大臣</span>
                <span class="basicInfo-item name">外文名</span>
                <span class="basicInfo-item value">Prime Minister of Japan</span>
            </div>
        </body>
        </html>
        "#;

        let result = parse_baike_html(html, "日本首相", "https://baike.baidu.com/item/日本首相");
        assert!(
            result.contains("日本首相"),
            "should contain title, got: {result}"
        );
        assert!(
            result.contains("政府首脑"),
            "should contain summary, got: {result}"
        );
        assert!(
            result.contains("中文名: 日本内阁总理大臣"),
            "should contain basic info, got: {result}"
        );
        assert!(
            result.contains("外文名: Prime Minister of Japan"),
            "should contain basic info, got: {result}"
        );
    }

    #[test]
    fn uses_meta_description_when_summary_missing() {
        let html = r#"
        <html>
        <head><meta name="description" content="Meta description fallback content here."></head>
        <body>
            <h1>Test Entity</h1>
            <div class="basicInfo">
                <span class="basicInfo-item name">类型</span>
                <span class="basicInfo-item value">概念</span>
            </div>
        </body>
        </html>
        "#;

        let result = parse_baike_html(html, "Test Entity", "https://baike.baidu.com/item/Test");
        assert!(
            result.contains("Meta description fallback"),
            "should use meta description, got: {result}"
        );
    }

    #[test]
    fn extracts_polysemy_when_no_summary_or_basic_info() {
        let html = r#"
        <html>
        <body>
            <h1>苹果</h1>
            <div class="polysemy-list">
                <ul>
                    <li><a href="/item/苹果（水果）">苹果（水果）</a></li>
                    <li><a href="/item/苹果公司">苹果公司</a></li>
                </ul>
            </div>
        </body>
        </html>
        "#;

        let result = parse_baike_html(html, "苹果", "https://baike.baidu.com/item/苹果");
        assert!(
            result.contains("多义词"),
            "should detect polysemy, got: {result}"
        );
        assert!(
            result.contains("苹果（水果）"),
            "should list polysemy items, got: {result}"
        );
        assert!(
            result.contains("苹果公司"),
            "should list polysemy items, got: {result}"
        );
    }

    #[test]
    fn formats_baike_result() {
        let html = r#"
        <html>
        <head><meta name="description" content="Summary text here."></head>
        <body>
            <h1>Test Keyword</h1>
            <div class="lemma-summary">A detailed summary of the keyword.</div>
            <div class="basicInfo">
                <span class="basicInfo-item name">属性</span>
                <span class="basicInfo-item value">测试</span>
            </div>
        </body>
        </html>
        "#;

        let result = parse_baike_html(
            html,
            "Test Keyword",
            "https://baike.baidu.com/item/TestKeyword",
        );
        assert!(
            result.starts_with("百度百科:"),
            "should start with baike prefix, got: {result}"
        );
        assert!(
            result.contains("链接: https://baike.baidu.com/item/TestKeyword"),
            "should contain link"
        );
        assert!(result.contains("摘要:"), "should have summary section");
        assert!(
            result.contains("基本信息:"),
            "should have basic info section"
        );
    }

    // ── Helpers ────────────────────────────────────────────────────────

    fn test_context() -> AgentContext {
        use crate::domain::agent::ToolDefinition;

        AgentContext {
            user_id: 1,
            conversation_id: None,
            recent_messages: vec![],
            summary: None,
            memories: vec![],
            rag_chunks: vec![],
            user_profile: None,
            location: None,
            tools: vec![ToolDefinition {
                name: "get_baidu_baike".into(),
                description: "search baidu baike".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "keyword": {"type": "string"}
                    },
                    "required": ["keyword"]
                }),
            }],
        }
    }
}
