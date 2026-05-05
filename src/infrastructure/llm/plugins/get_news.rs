use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::warn;

use crate::domain::llm::tools::{LlmTool, ToolExecutionContext, ToolOutcome};

/// News tool powered by [NewsAPI](https://newsapi.org/) (or a compatible service).
///
/// Requires an API key set via:
/// 1. `config.toml` → `[plugins].news_api_key`
/// 2. Environment variable `NEWS_API_KEY`
///
/// If no key is configured the tool returns a clear configuration hint
/// so the LLM can explain it to the user.
pub struct GetNewsTool {
    client: reqwest::Client,
    api_key: Option<String>,
    base_url: String,
}

impl GetNewsTool {
    /// `api_key` — optional key for the news service.
    /// `base_url` — root URL (defaults to `https://newsapi.org/v2`).
    pub fn new(api_key: Option<String>, base_url: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("news reqwest client"),
            api_key,
            base_url,
        }
    }
}

#[async_trait]
impl LlmTool for GetNewsTool {
    fn name(&self) -> &str {
        "get_news"
    }

    fn tool_definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "get_news",
                "description": "Get the latest news headlines. Use when the user asks for news, current events, or what's happening in the world. Supports filtering by topic.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "description": "Optional topic to filter, e.g. 'technology', 'sports', 'health', 'science'. Omit for general headlines."
                        },
                        "count": {
                            "type": "integer",
                            "description": "Number of headlines to return (1-5, default 3)."
                        }
                    },
                    "required": []
                }
            }
        })
    }

    async fn invoke(&self, _context: &mut ToolExecutionContext, arguments: &Value) -> ToolOutcome {
        let topic = arguments
            .get("topic")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let count = arguments
            .get("count")
            .and_then(|v| v.as_i64())
            .unwrap_or(3)
            .clamp(1, 5) as usize;

        let api_key = match &self.api_key {
            Some(k) => k.clone(),
            None => {
                return ToolOutcome::continue_(
                    "News API key is not configured. Set NEWS_API_KEY environment variable \
                     or add [plugins].news_api_key to config.toml. \
                     Get a free key at https://newsapi.org/register."
                        .to_string(),
                );
            }
        };

        match fetch_news(
            &self.client,
            &self.base_url,
            &api_key,
            topic.as_deref(),
            count,
        )
        .await
        {
            Ok(summary) => ToolOutcome::continue_(summary),
            Err(err) => {
                warn!(error = %err, "news lookup failed");
                ToolOutcome::continue_(format!("Sorry, unable to fetch news right now ({err})."))
            }
        }
    }
}

// ── NewsAPI response types ──

#[derive(Debug, Deserialize)]
struct NewsApiResponse {
    status: String,
    #[serde(default)]
    articles: Vec<NewsArticle>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NewsArticle {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    source: Option<ArticleSource>,
}

#[derive(Debug, Deserialize)]
struct ArticleSource {
    name: String,
}

// ── Fetch helper ──

async fn fetch_news(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    topic: Option<&str>,
    count: usize,
) -> Result<String, String> {
    let base = base_url.trim_end_matches('/');

    // Build query — use /top-headlines for general, /everything for topic search
    let (endpoint, query_param) = if let Some(t) = topic {
        ("everything", Some(("q", t.to_string())))
    } else {
        ("top-headlines", None::<(&str, String)>)
    };

    let mut url = format!("{base}/{endpoint}?apiKey={api_key}&pageSize={count}&language=en");

    if let Some((key, value)) = query_param {
        url.push_str(&format!("&{key}={}", urlencoding(&value)));
    }

    let resp: NewsApiResponse = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("news request failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("news parse: {e}"))?;

    if resp.status != "ok" {
        let msg = resp.message.as_deref().unwrap_or("unknown error");
        return Err(format!("news API error: {msg}"));
    }

    if resp.articles.is_empty() {
        return Ok("No news articles found for this query.".to_string());
    }

    let topic_label = topic
        .map(|t| {
            let mut chars = t.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
            }
        })
        .unwrap_or_else(|| "Top Stories".to_string());

    let mut result = format!("=== {topic_label} News ===\n");
    for (i, article) in resp.articles.iter().take(count).enumerate() {
        let source = article
            .source
            .as_ref()
            .map(|s| s.name.as_str())
            .unwrap_or("Unknown");
        let desc = article.description.as_deref().unwrap_or("(no description)");
        result.push_str(&format!(
            "{}. [{}] {} — {}\n",
            i + 1,
            source,
            article.title,
            desc
        ));
    }

    Ok(result)
}

/// Manual url-encode so we don't need a whole crate for one function.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}
