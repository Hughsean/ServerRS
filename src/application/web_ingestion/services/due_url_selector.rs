//! Due-URL selection (task-book §5.3 #7, §16.1 #10).
//!
//! A URL is "due" for crawling when it has never been crawled, or when enough
//! time has elapsed since its last crawl. Disabled URLs are never due.

use chrono::{DateTime, Utc};

use crate::domain::web_ingestion::repository::WebSourceUrl;

/// Whether `url` is due for crawling at `now`.
pub fn is_due(url: &WebSourceUrl, now: DateTime<Utc>) -> bool {
    if !url.enabled {
        return false;
    }
    if url.deleted_at.is_some() {
        return false;
    }
    match url.last_crawled_at {
        None => true,
        Some(last) => (now - last).num_seconds() >= url.crawl_interval_secs as i64,
    }
}

/// Filter a list of URLs down to the ones currently due.
pub fn select_due(urls: Vec<WebSourceUrl>, now: DateTime<Utc>) -> Vec<WebSourceUrl> {
    urls.into_iter().filter(|u| is_due(u, now)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn url(enabled: bool, last: Option<DateTime<Utc>>, interval: u32) -> WebSourceUrl {
        WebSourceUrl {
            id: 1,
            source_id: 1,
            url: "https://example.com".into(),
            canonical_url: None,
            url_hash: "h".into(),
            enabled,
            crawl_interval_secs: interval,
            last_crawled_at: last,
            last_content_hash: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
        }
    }

    #[test]
    fn never_crawled_is_due() {
        assert!(is_due(&url(true, None, 86400), Utc::now()));
    }

    #[test]
    fn disabled_is_never_due() {
        assert!(!is_due(&url(false, None, 86400), Utc::now()));
    }

    #[test]
    fn recently_crawled_not_due() {
        let now = Utc::now();
        let last = now - Duration::seconds(100);
        assert!(!is_due(&url(true, Some(last), 86400), now));
    }

    #[test]
    fn elapsed_interval_is_due() {
        let now = Utc::now();
        let last = now - Duration::seconds(90000);
        assert!(is_due(&url(true, Some(last), 86400), now));
    }
}
