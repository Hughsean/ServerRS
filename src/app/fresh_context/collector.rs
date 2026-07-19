use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use scraper::{ElementRef, Html, Selector};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::warn;

use crate::app::fresh_context::config::FreshContextUseCaseConfig;
use crate::app::fresh_context::policy::FreshContextPolicy;
use crate::app::web_ingestion::services::html_cleaner;
use crate::domain::fresh_context::{
    FreshContentFetcher, FreshContextRepoT, FreshFetchResult, FreshSource, NewFreshItem,
    fresh_status, rumor_level, source_kind,
};
use crate::shared::error::AppError;

const MIN_FRESH_CLEAN_CHARS: usize = 12;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FreshCollectStats {
    pub sources_seen: usize,
    pub sources_collected: usize,
    pub sources_failed: usize,
    pub items_seen: usize,
    pub items_inserted: usize,
    pub items_duplicated: usize,
    pub items_skipped_short: usize,
}

#[derive(Debug, Clone)]
struct CandidateFreshItem {
    url: Option<String>,
    canonical_url: Option<String>,
    title: Option<String>,
    raw_text: Option<String>,
    clean_text: String,
    published_at: Option<DateTime<Utc>>,
    heat_score: Option<f64>,
    metadata: serde_json::Value,
}

pub struct FreshCollectorService {
    repo: Arc<dyn FreshContextRepoT>,
    fetcher: Arc<dyn FreshContentFetcher>,
    policy: FreshContextPolicy,
    config: FreshContextUseCaseConfig,
    last_attempted_at: Mutex<HashMap<u64, DateTime<Utc>>>,
}

impl FreshCollectorService {
    pub fn new(
        repo: Arc<dyn FreshContextRepoT>,
        fetcher: Arc<dyn FreshContentFetcher>,
        config: FreshContextUseCaseConfig,
    ) -> Self {
        Self {
            repo,
            fetcher,
            policy: FreshContextPolicy::new(config.clone()),
            config,
            last_attempted_at: Mutex::new(HashMap::new()),
        }
    }

    pub async fn collect_tick(&self) -> Result<FreshCollectStats, AppError> {
        let tick_started_at = Utc::now();
        let sources = self
            .repo
            .list_enabled_sources(self.config.max_sources_per_tick as u64)
            .await?;
        let mut total = FreshCollectStats {
            sources_seen: sources.len(),
            ..FreshCollectStats::default()
        };

        for source in sources {
            if !self.policy.source_is_eligible(&source) {
                continue;
            }
            if !self.source_is_due(&source, tick_started_at).await {
                continue;
            }
            match self.collect_source(&source).await {
                Ok(stats) => total.merge(stats),
                Err(error) => {
                    total.sources_failed += 1;
                    warn!(
                        source_id = source.id,
                        source = %source.name,
                        error = %error,
                        "Fresh Context source collection failed"
                    );
                }
            }
        }

        Ok(total)
    }

    async fn source_is_due(&self, source: &FreshSource, now: DateTime<Utc>) -> bool {
        let interval = ChronoDuration::seconds(source.crawl_interval_secs.max(1) as i64);
        let mut last_attempted_at = self.last_attempted_at.lock().await;
        if let Some(last) = last_attempted_at.get(&source.id) {
            if now.signed_duration_since(*last) < interval {
                return false;
            }
        }
        last_attempted_at.insert(source.id, now);
        true
    }

    async fn collect_source(&self, source: &FreshSource) -> Result<FreshCollectStats, AppError> {
        let base_url = source
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .ok_or_else(|| {
                AppError::Validation(format!("fresh source '{}' missing base_url", source.name))
            })?;
        let allowed_domains = allowed_domains(source)?;
        let fetched = self
            .fetcher
            .fetch(base_url, allowed_domains.as_deref())
            .await?;
        let fetched_at = Utc::now();
        let candidates = candidates_from_source(source, &fetched, fetched_at)?
            .into_iter()
            .take(self.config.max_items_per_source)
            .collect::<Vec<_>>();

        let mut stats = FreshCollectStats {
            sources_collected: 1,
            items_seen: candidates.len(),
            ..FreshCollectStats::default()
        };

        for candidate in candidates {
            match self.insert_candidate(source, candidate, fetched_at).await? {
                InsertCandidateOutcome::Inserted => stats.items_inserted += 1,
                InsertCandidateOutcome::Duplicate => stats.items_duplicated += 1,
                InsertCandidateOutcome::TooShort => stats.items_skipped_short += 1,
            }
        }

        Ok(stats)
    }

    async fn insert_candidate(
        &self,
        source: &FreshSource,
        candidate: CandidateFreshItem,
        fetched_at: DateTime<Utc>,
    ) -> Result<InsertCandidateOutcome, AppError> {
        if candidate.clean_text.chars().count() < MIN_FRESH_CLEAN_CHARS {
            return Ok(InsertCandidateOutcome::TooShort);
        }

        let content_hash = fresh_content_hash(&candidate);
        if self
            .repo
            .find_item_by_source_content(source.id, &content_hash)
            .await?
            .is_some()
        {
            return Ok(InsertCandidateOutcome::Duplicate);
        }

        let url_hash = candidate
            .canonical_url
            .as_ref()
            .map(|url| sha256_hex(url.trim()));
        let expires_at = expires_at_for_source(&self.policy, fetched_at, source);
        let heat_score = candidate.heat_score.unwrap_or(0.0).clamp(0.0, 1.0);
        let item = NewFreshItem {
            source_id: source.id,
            url: candidate.url,
            canonical_url: candidate.canonical_url,
            url_hash,
            title: candidate.title,
            raw_text: candidate.raw_text,
            clean_text: Some(candidate.clean_text),
            summary: None,
            published_at: candidate.published_at,
            fetched_at,
            expires_at,
            content_hash,
            status: fresh_status::FETCHED.into(),
            reliability_score: source.reliability_score,
            freshness_score: 0.5,
            heat_score,
            rumor_level: initial_rumor_level(&source.source_kind).into(),
            risk_flags: None,
            metadata: Some(candidate.metadata),
        };
        let _ = self.repo.insert_item(item).await?;
        Ok(InsertCandidateOutcome::Inserted)
    }
}

enum InsertCandidateOutcome {
    Inserted,
    Duplicate,
    TooShort,
}

impl FreshCollectStats {
    fn merge(&mut self, other: FreshCollectStats) {
        self.sources_collected += other.sources_collected;
        self.sources_failed += other.sources_failed;
        self.items_seen += other.items_seen;
        self.items_inserted += other.items_inserted;
        self.items_duplicated += other.items_duplicated;
        self.items_skipped_short += other.items_skipped_short;
    }
}

fn candidates_from_source(
    source: &FreshSource,
    fetched: &FreshFetchResult,
    fetched_at: DateTime<Utc>,
) -> Result<Vec<CandidateFreshItem>, AppError> {
    if collector_kind(source) == Some("json_list") {
        return parse_json_list_items(source, fetched, fetched_at);
    }

    if source.source_kind == source_kind::RSS || collector_kind(source) == Some("rss") {
        return Ok(parse_feed_items(&fetched.body_text)
            .into_iter()
            .map(|item| candidate_from_feed_item(item, fetched))
            .collect());
    }

    if collector_kind(source) == Some("html_list") || source_prefers_list_parse(source) {
        let candidates = parse_html_list_items(source, fetched, fetched_at)?;
        if !candidates.is_empty() {
            return Ok(candidates);
        }
    }

    Ok(vec![candidate_from_page(source, fetched, fetched_at)])
}

fn candidate_from_page(
    source: &FreshSource,
    fetched: &FreshFetchResult,
    fetched_at: DateTime<Utc>,
) -> CandidateFreshItem {
    let (title, clean_text) = clean_body_text(&fetched.body_text, fetched.content_type.as_deref());
    CandidateFreshItem {
        url: Some(fetched.final_url.clone()),
        canonical_url: Some(fetched.final_url.clone()),
        title: prefer_text(Some(title), source.name.as_str()),
        raw_text: Some(fetched.body_text.clone()),
        clean_text,
        published_at: None,
        heat_score: None,
        metadata: json!({
            "collector": "fresh_context",
            "collector_kind": "page",
            "fetched_at": fetched_at.to_rfc3339(),
            "content_type": fetched.content_type,
            "content_length": fetched.content_length,
        }),
    }
}

fn candidate_from_feed_item(
    item: ParsedFeedItem,
    fetched: &FreshFetchResult,
) -> CandidateFreshItem {
    let clean_text = clean_text_fragment(&item.body);
    let title = item.title.filter(|title| !title.trim().is_empty());
    let canonical_url = item.link.or_else(|| Some(fetched.final_url.clone()));
    CandidateFreshItem {
        url: canonical_url.clone(),
        canonical_url,
        title,
        raw_text: Some(item.body),
        clean_text,
        published_at: item.published_at,
        heat_score: None,
        metadata: json!({
            "collector": "fresh_context",
            "collector_kind": "rss",
            "feed_url": fetched.final_url,
        }),
    }
}

fn parse_html_list_items(
    source: &FreshSource,
    fetched: &FreshFetchResult,
    fetched_at: DateTime<Utc>,
) -> Result<Vec<CandidateFreshItem>, AppError> {
    let document = Html::parse_document(&fetched.body_text);
    let item_selector = metadata_str(source, "item_selector")
        .unwrap_or("article, li, .item, .entry, .result, .hot-item");
    let selector = Selector::parse(item_selector).map_err(|e| {
        AppError::Validation(format!(
            "fresh source '{}' item_selector is invalid: {e}",
            source.name
        ))
    })?;

    let mut candidates = Vec::new();
    for (index, node) in document.select(&selector).enumerate() {
        let title = selected_text(
            node,
            metadata_str(source, "title_selector").unwrap_or("a, h1, h2, h3, .title"),
        )
        .or_else(|| Some(normalize_text(&node.text().collect::<Vec<_>>().join(" "))))
        .filter(|value| !value.is_empty());
        let link = selected_attr(
            node,
            metadata_str(source, "link_selector").unwrap_or("a[href]"),
            metadata_str(source, "link_attr").unwrap_or("href"),
        )
        .and_then(|url| resolve_url(&fetched.final_url, &url));
        let summary = selected_text(
            node,
            metadata_str(source, "summary_selector")
                .unwrap_or("p, .summary, .desc, .description, .content"),
        );
        let published_at = selected_attr(
            node,
            metadata_str(source, "published_at_selector").unwrap_or("time"),
            metadata_str(source, "published_at_attr").unwrap_or("datetime"),
        )
        .or_else(|| {
            selected_text(
                node,
                metadata_str(source, "published_at_selector").unwrap_or("time"),
            )
        })
        .and_then(|raw| parse_feed_datetime(&raw));
        let heat_score = selected_text(
            node,
            metadata_str(source, "heat_selector").unwrap_or(".heat, .score, .hot, .rank"),
        )
        .and_then(|raw| parse_heat_score(&raw));

        let clean_text = normalize_text(&format!(
            "{} {}",
            title.as_deref().unwrap_or(""),
            summary.as_deref().unwrap_or("")
        ));
        if clean_text.chars().count() < MIN_FRESH_CLEAN_CHARS {
            continue;
        }

        candidates.push(CandidateFreshItem {
            url: link.clone(),
            canonical_url: link,
            title,
            raw_text: Some(node.html()),
            clean_text,
            published_at,
            heat_score,
            metadata: json!({
                "collector": "fresh_context",
                "collector_kind": "html_list",
                "source_kind": source.source_kind,
                "source_index": index,
                "fetched_at": fetched_at.to_rfc3339(),
                "list_url": fetched.final_url,
            }),
        });
    }
    Ok(candidates)
}

fn parse_json_list_items(
    source: &FreshSource,
    fetched: &FreshFetchResult,
    fetched_at: DateTime<Utc>,
) -> Result<Vec<CandidateFreshItem>, AppError> {
    let data: serde_json::Value = serde_json::from_str(&fetched.body_text).map_err(|e| {
        AppError::Validation(format!(
            "fresh source '{}' returned invalid json list: {e}",
            source.name
        ))
    })?;
    let items_path = metadata_str(source, "items_path").unwrap_or("");
    let Some(items) = json_path(&data, items_path).and_then(|value| value.as_array()) else {
        return Ok(Vec::new());
    };

    let title_field = metadata_str(source, "title_field").unwrap_or("title");
    let url_field = metadata_str(source, "url_field").unwrap_or("url");
    let summary_field = metadata_str(source, "summary_field").unwrap_or("summary");
    let content_field = metadata_str(source, "content_field").unwrap_or("content");
    let published_at_field = metadata_str(source, "published_at_field").unwrap_or("published_at");
    let heat_field = metadata_str(source, "heat_field").unwrap_or("heat");

    let mut candidates = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let title = field_string(item, title_field);
        let summary =
            field_string(item, summary_field).or_else(|| field_string(item, content_field));
        let url =
            field_string(item, url_field).and_then(|url| resolve_url(&fetched.final_url, &url));
        let published_at =
            field_string(item, published_at_field).and_then(|raw| parse_feed_datetime(&raw));
        let heat_score = field_number(item, heat_field)
            .or_else(|| field_string(item, heat_field).and_then(|raw| parse_heat_score(&raw)));
        let clean_text = normalize_text(&format!(
            "{} {}",
            title.as_deref().unwrap_or(""),
            summary.as_deref().unwrap_or("")
        ));
        if clean_text.chars().count() < MIN_FRESH_CLEAN_CHARS {
            continue;
        }

        candidates.push(CandidateFreshItem {
            url: url.clone(),
            canonical_url: url,
            title,
            raw_text: Some(item.to_string()),
            clean_text,
            published_at,
            heat_score,
            metadata: json!({
                "collector": "fresh_context",
                "collector_kind": "json_list",
                "source_kind": source.source_kind,
                "source_index": index,
                "fetched_at": fetched_at.to_rfc3339(),
                "list_url": fetched.final_url,
            }),
        });
    }
    Ok(candidates)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedFeedItem {
    title: Option<String>,
    link: Option<String>,
    body: String,
    published_at: Option<DateTime<Utc>>,
}

fn parse_feed_items(feed_xml: &str) -> Vec<ParsedFeedItem> {
    let rss_items = rss_item_fragments(feed_xml);
    if !rss_items.is_empty() {
        return rss_items
            .into_iter()
            .filter_map(|fragment| parse_rss_item_fragment(&fragment))
            .collect();
    }

    let document = Html::parse_document(feed_xml);
    select_all(&document, "entry")
        .into_iter()
        .filter_map(parse_atom_entry)
        .collect()
}

fn rss_item_fragments(feed_xml: &str) -> Vec<String> {
    let regex =
        regex::Regex::new(r"(?is)<item(?:\s[^>]*)?>(.*?)</item>").expect("static regex compiles");
    regex
        .captures_iter(feed_xml)
        .filter_map(|captures| captures.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

fn parse_rss_item_fragment(raw: &str) -> Option<ParsedFeedItem> {
    let title = tag_text(raw, "title");
    let link = tag_text(raw, "link");
    let body = tag_text(&raw, "description")
        .or_else(|| tag_text(raw, "summary"))
        .or_else(|| tag_text(raw, "content"))
        .or_else(|| title.clone())
        .unwrap_or_default();
    if body.trim().is_empty() && title.as_deref().unwrap_or("").trim().is_empty() {
        return None;
    }

    Some(ParsedFeedItem {
        title,
        link,
        body,
        published_at: tag_text(&raw, "pubDate")
            .or_else(|| tag_text(raw, "published"))
            .or_else(|| tag_text(raw, "updated"))
            .or_else(|| tag_text(raw, "date"))
            .and_then(|raw| parse_feed_datetime(&raw)),
    })
}

fn parse_atom_entry(entry: ElementRef<'_>) -> Option<ParsedFeedItem> {
    let title = first_text(entry, &["title"]);
    let link = first_link(entry).or_else(|| first_text(entry, &["link"]));
    let body = first_text(entry, &["summary", "content", "description"])
        .or_else(|| title.clone())
        .unwrap_or_default();
    if body.trim().is_empty() && title.as_deref().unwrap_or("").trim().is_empty() {
        return None;
    }

    Some(ParsedFeedItem {
        title,
        link,
        body,
        published_at: first_text(entry, &["published", "updated"])
            .and_then(|raw| parse_feed_datetime(&raw)),
    })
}

fn select_all<'a>(document: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    let selector = Selector::parse(selector).expect("static selector must parse");
    document.select(&selector).collect()
}

fn first_text(root: ElementRef<'_>, selectors: &[&str]) -> Option<String> {
    for selector in selectors {
        let selector = Selector::parse(selector).expect("static selector must parse");
        if let Some(value) = root
            .select(&selector)
            .next()
            .map(|node| normalize_text(&node.text().collect::<Vec<_>>().join(" ")))
            .filter(|value| !value.is_empty())
        {
            return Some(value);
        }
    }
    None
}

fn first_link(root: ElementRef<'_>) -> Option<String> {
    let selector = Selector::parse("link").expect("static selector must parse");
    root.select(&selector).find_map(|node| {
        node.value()
            .attr("href")
            .map(normalize_text)
            .filter(|value| !value.is_empty())
    })
}

fn selected_text(root: ElementRef<'_>, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    root.select(&selector)
        .next()
        .map(|node| normalize_text(&node.text().collect::<Vec<_>>().join(" ")))
        .filter(|value| !value.is_empty())
}

fn selected_attr(root: ElementRef<'_>, selector: &str, attr: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    root.select(&selector)
        .find_map(|node| node.value().attr(attr).map(normalize_text))
        .filter(|value| !value.is_empty())
}

fn tag_text(fragment: &str, tag: &str) -> Option<String> {
    let pattern = format!(r"(?is)<{tag}(?:\s[^>]*)?>(.*?)</{tag}>");
    let regex = regex::Regex::new(&pattern).ok()?;
    let captures = regex.captures(fragment)?;
    let raw = captures.get(1)?.as_str();
    Some(normalize_text(&strip_cdata(raw))).filter(|value| !value.is_empty())
}

fn strip_cdata(input: &str) -> String {
    let trimmed = input.trim();
    trimmed
        .strip_prefix("<![CDATA[")
        .and_then(|value| value.strip_suffix("]]>"))
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

fn clean_body_text(body: &str, content_type: Option<&str>) -> (String, String) {
    let is_html = content_type
        .map(|ct| ct.contains("html") || ct.contains("xml"))
        .unwrap_or_else(|| body.contains('<') && body.contains('>'));
    if is_html {
        html_cleaner::clean(body)
    } else {
        (String::new(), normalize_text(body))
    }
}

fn clean_text_fragment(input: &str) -> String {
    if input.contains('<') && input.contains('>') {
        let (_, clean_text) = html_cleaner::clean(input);
        clean_text
    } else {
        normalize_text(input)
    }
}

fn parse_feed_datetime(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    chrono::DateTime::parse_from_rfc2822(raw)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(raw))
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

fn allowed_domains(source: &FreshSource) -> Result<Option<Vec<String>>, AppError> {
    let Some(value) = &source.allowed_domains else {
        return Ok(None);
    };
    let domains = value.as_array().ok_or_else(|| {
        AppError::Validation(format!(
            "fresh source '{}' allowed_domains must be a string array",
            source.name
        ))
    })?;
    let domains = domains
        .iter()
        .filter_map(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    Ok(if domains.is_empty() {
        None
    } else {
        Some(domains)
    })
}

fn collector_kind(source: &FreshSource) -> Option<&str> {
    metadata_str(source, "collector_kind")
}

fn metadata_str<'a>(source: &'a FreshSource, key: &str) -> Option<&'a str> {
    source
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(key))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn source_prefers_list_parse(source: &FreshSource) -> bool {
    matches!(
        source.source_kind.as_str(),
        source_kind::TREND
            | source_kind::GOSSIP
            | source_kind::FORUM
            | source_kind::SOCIAL
            | source_kind::SEARCH
    )
}

fn json_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    if path.trim().is_empty() {
        return Some(value);
    }
    let mut current = value;
    for segment in path.split('.').map(str::trim).filter(|s| !s.is_empty()) {
        current = current.get(segment)?;
    }
    Some(current)
}

fn field_string(value: &serde_json::Value, field: &str) -> Option<String> {
    json_path(value, field)
        .and_then(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .or_else(|| value.as_i64().map(|n| n.to_string()))
                .or_else(|| value.as_u64().map(|n| n.to_string()))
                .or_else(|| value.as_f64().map(|n| n.to_string()))
        })
        .map(|value| normalize_text(&value))
        .filter(|value| !value.is_empty())
}

fn field_number(value: &serde_json::Value, field: &str) -> Option<f64> {
    json_path(value, field).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|n| n as f64))
            .or_else(|| value.as_u64().map(|n| n as f64))
    })
}

fn parse_heat_score(raw: &str) -> Option<f64> {
    let number = raw
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>()
        .parse::<f64>()
        .ok()?;
    if number <= 1.0 {
        Some(number.clamp(0.0, 1.0))
    } else {
        Some((number / 100.0).clamp(0.0, 1.0))
    }
}

fn resolve_url(base_url: &str, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('#') || raw.starts_with("javascript:") {
        return None;
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    if let Some(rest) = raw.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    let scheme_end = base_url.find("://")?;
    let after_scheme = &base_url[scheme_end + 3..];
    let host_end = after_scheme.find('/').map(|idx| scheme_end + 3 + idx);
    let origin = match host_end {
        Some(end) => &base_url[..end],
        None => base_url,
    };
    if raw.starts_with('/') {
        return Some(format!("{origin}{raw}"));
    }
    let base_dir = base_url
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or(origin);
    Some(format!("{}/{}", base_dir.trim_end_matches('/'), raw))
}

fn expires_at_for_source(
    policy: &FreshContextPolicy,
    fetched_at: DateTime<Utc>,
    source: &FreshSource,
) -> DateTime<Utc> {
    if source.default_ttl_secs > 0 {
        fetched_at + chrono::Duration::seconds(source.default_ttl_secs as i64)
    } else {
        policy.expires_at(fetched_at, &source.source_kind)
    }
}

fn initial_rumor_level(source_kind: &str) -> &'static str {
    match source_kind {
        source_kind::GOSSIP | source_kind::FORUM | source_kind::SOCIAL => rumor_level::RUMOR,
        _ => rumor_level::REPORTED,
    }
}

fn fresh_content_hash(candidate: &CandidateFreshItem) -> String {
    sha256_hex(&format!(
        "{}|{}|{}",
        candidate.title.as_deref().unwrap_or(""),
        candidate
            .published_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
        candidate.clean_text
    ))
}

fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

fn prefer_text(value: Option<String>, fallback: &str) -> Option<String> {
    value
        .map(|value| normalize_text(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let fallback = normalize_text(fallback);
            if fallback.is_empty() {
                None
            } else {
                Some(fallback)
            }
        })
}

fn normalize_text(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_trait::async_trait;
    use tokio::sync::Mutex;

    use super::*;
    use crate::domain::fresh_context::{
        FreshChunk, FreshItem, FreshItemDistillUpdate, FreshTopic, FreshTopicEvidence,
        NewFreshChunk, NewFreshSource, NewFreshTopic, NewFreshTopicEvidence,
    };

    #[test]
    fn parses_rss_items() {
        let rss = r#"
        <rss><channel>
          <item>
            <title>新闻标题</title>
            <link>https://example.com/a</link>
            <description><![CDATA[<p>新闻正文内容足够长，可以进入后续处理。</p>]]></description>
            <pubDate>Sun, 28 Jun 2026 10:00:00 GMT</pubDate>
          </item>
        </channel></rss>"#;

        let items = parse_feed_items(rss);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title.as_deref(), Some("新闻标题"));
        assert_eq!(items[0].link.as_deref(), Some("https://example.com/a"));
        assert!(items[0].published_at.is_some());
    }

    #[test]
    fn parses_atom_entry_link_href() {
        let atom = r#"
        <feed>
          <entry>
            <title>Atom 标题</title>
            <link href="https://example.com/atom-a" />
            <summary>Atom 摘要内容足够长，可以进入后续处理。</summary>
            <updated>2026-06-28T10:00:00Z</updated>
          </entry>
        </feed>"#;

        let items = parse_feed_items(atom);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].link.as_deref(), Some("https://example.com/atom-a"));
        assert!(items[0].published_at.is_some());
    }

    #[test]
    fn gossip_sources_start_as_rumor() {
        assert_eq!(initial_rumor_level(source_kind::GOSSIP), rumor_level::RUMOR);
        assert_eq!(initial_rumor_level(source_kind::RSS), rumor_level::REPORTED);
    }

    #[test]
    fn parses_html_list_items_with_metadata_selectors() {
        let mut source = test_source();
        source.source_kind = source_kind::TREND.into();
        source.metadata = Some(json!({
            "collector_kind": "html_list",
            "item_selector": ".hot-item",
            "title_selector": "a",
            "link_selector": "a",
            "summary_selector": ".desc",
            "heat_selector": ".heat"
        }));
        let fetched = FreshFetchResult {
            final_url: "https://example.com/hot/index.html".into(),
            content_type: Some("text/html".into()),
            body_text: r#"
                <div class="hot-item">
                  <a href="/news/a">热榜标题 A</a>
                  <span class="desc">这是一条热榜摘要，长度足够进入 Fresh Context。</span>
                  <span class="heat">88</span>
                </div>
            "#
            .into(),
            content_length: None,
        };

        let items = candidates_from_source(&source, &fetched, Utc::now()).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title.as_deref(), Some("热榜标题 A"));
        assert_eq!(
            items[0].canonical_url.as_deref(),
            Some("https://example.com/news/a")
        );
        assert_eq!(items[0].heat_score, Some(0.88));
    }

    #[test]
    fn parses_json_list_items_with_metadata_paths() {
        let mut source = test_source();
        source.source_kind = source_kind::SEARCH.into();
        source.metadata = Some(json!({
            "collector_kind": "json_list",
            "items_path": "data.items",
            "title_field": "name",
            "url_field": "link",
            "summary_field": "snippet",
            "published_at_field": "time",
            "heat_field": "score"
        }));
        let fetched = FreshFetchResult {
            final_url: "https://example.com/api/search".into(),
            content_type: Some("application/json".into()),
            body_text: r#"{
              "data": {
                "items": [
                  {
                    "name": "搜索结果标题",
                    "link": "https://example.com/result",
                    "snippet": "搜索结果摘要内容足够长，可以进入 Fresh Context。",
                    "time": "2026-06-28T10:00:00Z",
                    "score": 0.7
                  }
                ]
              }
            }"#
            .into(),
            content_length: None,
        };

        let items = candidates_from_source(&source, &fetched, Utc::now()).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title.as_deref(), Some("搜索结果标题"));
        assert_eq!(items[0].heat_score, Some(0.7));
        assert!(items[0].published_at.is_some());
    }

    #[tokio::test]
    async fn collector_fetches_rss_and_inserts_new_items() {
        let source = test_source();
        let repo = Arc::new(MockFreshRepo::new(vec![source]));
        let fetcher = Arc::new(MockFreshFetcher::new(HashMap::from([(
            "https://example.com/feed.xml".into(),
            FreshFetchResult {
                final_url: "https://example.com/feed.xml".into(),
                content_type: Some("application/xml".into()),
                body_text: r#"
                  <rss><channel><item>
                    <title>新闻标题</title>
                    <link>https://example.com/a</link>
                    <description>新闻正文内容足够长，可以进入后续处理。</description>
                  </item></channel></rss>
                "#
                .into(),
                content_length: None,
            },
        )])));
        let collector =
            FreshCollectorService::new(repo.clone(), fetcher, FreshContextUseCaseConfig::default());

        let stats = collector.collect_tick().await.unwrap();
        assert_eq!(stats.items_seen, 1);
        assert_eq!(stats.items_inserted, 1);
        assert_eq!(repo.inserted_count().await, 1);
    }

    #[tokio::test]
    async fn collector_respects_source_crawl_interval_in_memory() {
        let source = test_source();
        let repo = Arc::new(MockFreshRepo::new(vec![source]));
        let fetcher = Arc::new(MockFreshFetcher::new(HashMap::from([(
            "https://example.com/feed.xml".into(),
            FreshFetchResult {
                final_url: "https://example.com/feed.xml".into(),
                content_type: Some("application/xml".into()),
                body_text: r#"
                  <rss><channel><item>
                    <title>新闻标题</title>
                    <link>https://example.com/a</link>
                    <description>新闻正文内容足够长，可以进入后续处理。</description>
                  </item></channel></rss>
                "#
                .into(),
                content_length: None,
            },
        )])));
        let collector =
            FreshCollectorService::new(repo.clone(), fetcher, FreshContextUseCaseConfig::default());

        let first = collector.collect_tick().await.unwrap();
        let second = collector.collect_tick().await.unwrap();

        assert_eq!(first.items_inserted, 1);
        assert_eq!(second.sources_seen, 1);
        assert_eq!(second.sources_collected, 0);
        assert_eq!(second.items_seen, 0);
        assert_eq!(repo.inserted_count().await, 1);
    }

    fn test_source() -> FreshSource {
        let now = Utc::now();
        FreshSource {
            id: 1,
            name: "测试 RSS".into(),
            source_kind: source_kind::RSS.into(),
            base_url: Some("https://example.com/feed.xml".into()),
            allowed_domains: Some(json!(["example.com"])),
            trust_level: "normal".into(),
            reliability_score: 0.8,
            crawl_interval_secs: 1800,
            default_ttl_secs: 86_400,
            risk_policy: "normal".into(),
            enabled: 1,
            metadata: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    struct MockFreshFetcher {
        responses: HashMap<String, FreshFetchResult>,
    }

    impl MockFreshFetcher {
        fn new(responses: HashMap<String, FreshFetchResult>) -> Self {
            Self { responses }
        }
    }

    #[async_trait]
    impl FreshContentFetcher for MockFreshFetcher {
        async fn fetch(
            &self,
            url: &str,
            _allowed_domains: Option<&[String]>,
        ) -> Result<FreshFetchResult, AppError> {
            self.responses
                .get(url)
                .cloned()
                .ok_or_else(|| AppError::NotFound(format!("missing mock response: {url}")))
        }
    }

    struct MockFreshRepo {
        sources: Vec<FreshSource>,
        inserted: Mutex<Vec<FreshItem>>,
    }

    impl MockFreshRepo {
        fn new(sources: Vec<FreshSource>) -> Self {
            Self {
                sources,
                inserted: Mutex::new(Vec::new()),
            }
        }

        async fn inserted_count(&self) -> usize {
            self.inserted.lock().await.len()
        }
    }

    #[async_trait]
    impl FreshContextRepoT for MockFreshRepo {
        async fn insert_source(&self, _source: NewFreshSource) -> Result<FreshSource, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn list_enabled_sources(&self, _limit: u64) -> Result<Vec<FreshSource>, AppError> {
            Ok(self.sources.clone())
        }

        async fn find_source_by_id(
            &self,
            _source_id: u64,
        ) -> Result<Option<FreshSource>, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn insert_item(&self, item: NewFreshItem) -> Result<FreshItem, AppError> {
            let id = self.inserted.lock().await.len() as u64 + 1;
            let now = Utc::now();
            let saved = FreshItem {
                id,
                source_id: item.source_id,
                url: item.url,
                canonical_url: item.canonical_url,
                url_hash: item.url_hash,
                title: item.title,
                raw_text: item.raw_text,
                clean_text: item.clean_text,
                summary: item.summary,
                published_at: item.published_at,
                fetched_at: item.fetched_at,
                expires_at: item.expires_at,
                content_hash: item.content_hash,
                status: item.status,
                reliability_score: item.reliability_score,
                freshness_score: item.freshness_score,
                heat_score: item.heat_score,
                rumor_level: item.rumor_level,
                risk_flags: item.risk_flags,
                metadata: item.metadata,
                created_at: now,
                updated_at: now,
                deleted_at: None,
            };
            self.inserted.lock().await.push(saved.clone());
            Ok(saved)
        }

        async fn find_item_by_source_content(
            &self,
            source_id: u64,
            content_hash: &str,
        ) -> Result<Option<FreshItem>, AppError> {
            Ok(self
                .inserted
                .lock()
                .await
                .iter()
                .find(|item| item.source_id == source_id && item.content_hash == content_hash)
                .cloned())
        }

        async fn find_item_by_id(&self, _item_id: u64) -> Result<Option<FreshItem>, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn list_active_items(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn list_chunkable_items(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn list_items_by_status(
            &self,
            _status: &str,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn expire_items(&self, _now: DateTime<Utc>) -> Result<u64, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn update_item_status_if_current(
            &self,
            _item_id: u64,
            _expected_status: &str,
            _new_status: &str,
            _metadata: Option<serde_json::Value>,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn update_item_distill_result_if_current(
            &self,
            _item_id: u64,
            _expected_status: &str,
            _new_status: &str,
            _update: FreshItemDistillUpdate,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn insert_topic(&self, _topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn upsert_topic(&self, _topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn find_topic_by_key(
            &self,
            _topic_key: &str,
        ) -> Result<Option<FreshTopic>, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn link_topic_evidence(
            &self,
            _evidence: NewFreshTopicEvidence,
        ) -> Result<FreshTopicEvidence, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn assign_topic_to_item_chunks(
            &self,
            _item_id: u64,
            _topic_id: u64,
        ) -> Result<u64, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn insert_chunks(
            &self,
            _chunks: &[NewFreshChunk],
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn find_chunk_by_id(&self, _chunk_id: u64) -> Result<Option<FreshChunk>, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn find_chunks_by_item(&self, _item_id: u64) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn list_indexable_chunks(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn mark_chunk_indexed(
            &self,
            _chunk_id: u64,
            _vector_id: String,
            _embedding_provider: String,
            _embedding_model: String,
            _embedding_dimension: u32,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn list_expired_indexed_chunks(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by collector tests")
        }

        async fn mark_chunk_vector_deleted(
            &self,
            _chunk_id: u64,
            _vector_id: &str,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by collector tests")
        }
    }
}
