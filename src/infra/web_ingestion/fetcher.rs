//! SSRF-safe HTTP fetcher for web ingestion.
//!
//! Guards: only http/https, blocks private/localhost/link-local/multicast IPs,
//! blocks metadata IPs (169.254.169.254), Content-Type allowlist, body size limit,
//! and revalidates every hop while following redirects manually.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, ACCEPT_LANGUAGE, CONTENT_TYPE, LOCATION, RETRY_AFTER,
};

use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::fetcher::{FetchResult, WebContentFetcher};
use crate::shared::config::WebIngestionConfig;

const MAX_REDIRECTS: usize = 5;
const ALLOWED_CONTENT_TYPES: &[&str] = &[
    "text/html",
    "text/plain",
    "application/xhtml+xml",
    "application/xml",
];

pub struct WebFetcher {
    client: reqwest::Client,
    max_body_bytes: u64,
    min_request_interval: Duration,
    request_jitter_ms: u64,
    next_request_at: Arc<tokio::sync::Mutex<HashMap<String, tokio::time::Instant>>>,
}

impl WebFetcher {
    pub fn new(config: &WebIngestionConfig) -> Result<Self, WebIngestionError> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            ACCEPT,
            reqwest::header::HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
            ),
        );
        headers.insert(
            ACCEPT_LANGUAGE,
            reqwest::header::HeaderValue::from_static("zh-CN,zh;q=0.9,en;q=0.8"),
        );
        headers.insert(
            ACCEPT_ENCODING,
            reqwest::header::HeaderValue::from_static("gzip, deflate"),
        );

        let mut client_builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(config.fetch_timeout_secs))
            .use_native_tls()
            .user_agent(config.fetch_user_agent.as_str())
            .default_headers(headers)
            .no_proxy();

        if !config.fetch_proxy_url.trim().is_empty() {
            let proxy = reqwest::Proxy::all(config.fetch_proxy_url.trim()).map_err(|e| {
                WebIngestionError::Internal(format!("invalid web ingestion proxy URL: {e}"))
            })?;
            client_builder = client_builder.proxy(proxy);
        }

        let client = client_builder.build().map_err(|e| {
            WebIngestionError::Internal(format!("failed to build HTTP client: {e}"))
        })?;
        Ok(Self {
            client,
            max_body_bytes: config.max_body_bytes,
            min_request_interval: Duration::from_millis(config.min_request_interval_ms),
            request_jitter_ms: config.request_jitter_ms,
            next_request_at: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        })
    }

    /// Fetch a URL with SSRF protection and optional domain allowlist.
    ///
    /// `allowed_domains` — when non-empty, the hostname must match one of the
    /// allowed domains (exact or subdomain).  Domain matching uses suffix-match:
    /// `sub.example.com` matches allowed `.example.com` or `example.com`.
    async fn fetch_inner(
        &self,
        url: &str,
        allowed_domains: Option<&[String]>,
    ) -> Result<FetchResult, WebIngestionError> {
        let mut current_url = url.to_string();
        let mut redirects_followed = 0usize;
        let response = loop {
            let parsed_url = validate_fetch_url(&current_url, allowed_domains).await?;
            self.wait_for_host(&parsed_url).await;
            let response = self
                .client
                .get(parsed_url.clone())
                .send()
                .await
                .map_err(|e| WebIngestionError::FetchFailed {
                    url: current_url.clone(),
                    reason: e.to_string(),
                })?;

            let status = response.status();
            if is_followable_redirect(status) {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .ok_or_else(|| WebIngestionError::FetchFailed {
                        url: current_url.clone(),
                        reason: format!("HTTP {status} without Location header"),
                    })?
                    .to_str()
                    .map_err(|e| WebIngestionError::FetchFailed {
                        url: current_url.clone(),
                        reason: format!("invalid redirect Location header: {e}"),
                    })?;
                let next_url =
                    parsed_url
                        .join(location)
                        .map_err(|e| WebIngestionError::FetchFailed {
                            url: current_url.clone(),
                            reason: format!("invalid redirect target '{location}': {e}"),
                        })?;

                if redirects_followed >= MAX_REDIRECTS {
                    return Err(WebIngestionError::FetchFailed {
                        url: current_url,
                        reason: format!("too many redirects (max {MAX_REDIRECTS})"),
                    });
                }
                redirects_followed += 1;
                current_url = next_url.to_string();
                continue;
            }

            break response;
        };

        let final_url = response.url().to_string();
        let status = response.status();
        if matches!(status.as_u16(), 429 | 503) {
            return Err(WebIngestionError::RateLimited {
                url: final_url,
                status: status.as_u16(),
                retry_after_secs: parse_retry_after(response.headers().get(RETRY_AFTER)),
            });
        }
        if !status.is_success() {
            return Err(WebIngestionError::FetchFailed {
                url: final_url,
                reason: format!("HTTP {status}"),
            });
        }

        // Content-Type check
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or(s).trim().to_lowercase());

        if let Some(ref ct) = content_type {
            if !ALLOWED_CONTENT_TYPES.contains(&ct.as_str()) {
                return Err(WebIngestionError::ContentTypeNotAllowed {
                    content_type: ct.clone(),
                });
            }
        } else {
            return Err(WebIngestionError::ContentTypeNotAllowed {
                content_type: "missing".into(),
            });
        }

        // Read body with size limit
        let content_length = response.content_length();
        if let Some(cl) = content_length {
            if cl > self.max_body_bytes {
                return Err(WebIngestionError::BodyTooLarge {
                    size: cl,
                    max: self.max_body_bytes,
                });
            }
        }

        let body = response
            .bytes()
            .await
            .map_err(|e| WebIngestionError::FetchFailed {
                url: final_url.clone(),
                reason: e.to_string(),
            })?;

        if body.len() as u64 > self.max_body_bytes {
            return Err(WebIngestionError::BodyTooLarge {
                size: body.len() as u64,
                max: self.max_body_bytes,
            });
        }

        let body_vec = body.to_vec();
        let body_text = String::from_utf8(body_vec.clone())
            .unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).into_owned());

        Ok(FetchResult {
            final_url,
            content_type,
            body: body_vec,
            body_text,
            content_length,
        })
    }

    async fn wait_for_host(&self, url: &reqwest::Url) {
        let Some(host) = url.host_str() else {
            return;
        };
        let host = host.to_ascii_lowercase();
        let now = tokio::time::Instant::now();
        let mut schedule = self.next_request_at.lock().await;
        let request_at = schedule.get(&host).copied().unwrap_or(now).max(now);
        let jitter = Duration::from_millis(jitter_ms(time_seed(), self.request_jitter_ms));
        schedule.insert(host, request_at + self.min_request_interval + jitter);
        drop(schedule);

        if request_at > now {
            tokio::time::sleep_until(request_at).await;
        }
    }
}

#[async_trait::async_trait]
impl WebContentFetcher for WebFetcher {
    async fn fetch(
        &self,
        url: &str,
        allowed_domains: Option<&[String]>,
    ) -> Result<FetchResult, WebIngestionError> {
        self.fetch_inner(url, allowed_domains).await
    }
}

fn time_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn jitter_ms(seed: u64, maximum_ms: u64) -> u64 {
    if maximum_ms == 0 {
        return 0;
    }
    seed % maximum_ms.saturating_add(1)
}

fn parse_retry_after(value: Option<&reqwest::header::HeaderValue>) -> Option<u64> {
    let raw = value?.to_str().ok()?.trim();
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(seconds);
    }

    let retry_at = chrono::DateTime::parse_from_rfc2822(raw)
        .ok()?
        .with_timezone(&chrono::Utc);
    let seconds = (retry_at - chrono::Utc::now()).num_seconds();
    Some(seconds.max(0) as u64)
}

async fn validate_fetch_url(
    raw: &str,
    allowed_domains: Option<&[String]>,
) -> Result<reqwest::Url, WebIngestionError> {
    let parsed = reqwest::Url::parse(raw.trim()).map_err(|e| WebIngestionError::SsrfRejected {
        url: raw.to_string(),
        reason: format!("invalid URL: {e}"),
    })?;
    let (scheme, host, port) = url_parts(&parsed, raw)?;

    if scheme != "http" && scheme != "https" {
        return Err(WebIngestionError::SsrfRejected {
            url: raw.to_string(),
            reason: format!("scheme not allowed: {scheme}"),
        });
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(WebIngestionError::SsrfRejected {
            url: raw.to_string(),
            reason: "URL userinfo is not allowed".into(),
        });
    }

    if let Some(allowed) = allowed_domains {
        if !allowed.is_empty() && !is_hostname_allowed(&host, allowed) {
            return Err(WebIngestionError::SsrfRejected {
                url: raw.to_string(),
                reason: format!(
                    "hostname '{host}' not in allowed_domains: {}",
                    allowed.join(", ")
                ),
            });
        }
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        validate_ip_for_url(ip, raw)?;
        return Ok(parsed);
    }

    let mut ips = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| WebIngestionError::SsrfRejected {
            url: raw.to_string(),
            reason: format!("DNS resolution failed: {e}"),
        })?;
    let mut resolved_any = false;
    for addr in ips.by_ref() {
        resolved_any = true;
        validate_ip_for_url(addr.ip(), raw)?;
    }
    if !resolved_any {
        return Err(WebIngestionError::SsrfRejected {
            url: raw.to_string(),
            reason: "DNS resolution returned no addresses".into(),
        });
    }

    Ok(parsed)
}

fn validate_ip_for_url(ip: IpAddr, url: &str) -> Result<(), WebIngestionError> {
    validate_ip(ip).map_err(|e| {
        if let WebIngestionError::SsrfRejected { reason, .. } = e {
            WebIngestionError::SsrfRejected {
                url: url.to_string(),
                reason,
            }
        } else {
            e
        }
    })
}

fn is_followable_redirect(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

/// Parse a URL with reqwest's structured URL implementation.
#[cfg(test)]
fn parse_url_parts(raw: &str) -> Result<(String, String, u16), WebIngestionError> {
    let parsed = reqwest::Url::parse(raw.trim()).map_err(|e| WebIngestionError::SsrfRejected {
        url: raw.to_string(),
        reason: format!("invalid URL: {e}"),
    })?;
    url_parts(&parsed, raw)
}

fn url_parts(parsed: &reqwest::Url, raw: &str) -> Result<(String, String, u16), WebIngestionError> {
    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = parsed
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| WebIngestionError::SsrfRejected {
            url: raw.to_string(),
            reason: "no host in URL".into(),
        })?;
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| WebIngestionError::SsrfRejected {
            url: raw.to_string(),
            reason: format!("URL has no known port for scheme '{scheme}'"),
        })?;

    Ok((scheme, host, port))
}

fn validate_ip(ip: IpAddr) -> Result<(), WebIngestionError> {
    match ip {
        IpAddr::V4(v4) => validate_ipv4(v4),
        IpAddr::V6(v6) => validate_ipv6(v6),
    }
}

fn validate_ipv4(ip: Ipv4Addr) -> Result<(), WebIngestionError> {
    let o = ip.octets();
    if o == [0, 0, 0, 0] {
        return Err(ssrf("IPv4 0.0.0.0"));
    }
    if o[0] == 127 {
        return Err(ssrf("IPv4 loopback"));
    }
    if o[0] == 10 {
        return Err(ssrf("IPv4 private (10.0.0.0/8)"));
    }
    if o[0] == 172 && o[1] >= 16 && o[1] <= 31 {
        return Err(ssrf("IPv4 private (172.16.0.0/12)"));
    }
    if o[0] == 192 && o[1] == 168 {
        return Err(ssrf("IPv4 private (192.168.0.0/16)"));
    }
    if o[0] == 169 && o[1] == 254 {
        return Err(ssrf("IPv4 link-local / metadata"));
    }
    if o[0] >= 224 && o[0] <= 239 {
        return Err(ssrf("IPv4 multicast"));
    }
    if o[0] >= 240 {
        return Err(ssrf("IPv4 reserved"));
    }
    Ok(())
}

fn validate_ipv6(ip: Ipv6Addr) -> Result<(), WebIngestionError> {
    if ip.is_loopback() {
        return Err(ssrf("IPv6 loopback"));
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return validate_ipv4(v4);
    }
    if ip.segments()[0] & 0xffc0 == 0xfe80 {
        return Err(ssrf("IPv6 link-local"));
    }
    if ip.segments()[0] & 0xfe00 == 0xfc00 {
        return Err(ssrf("IPv6 unique-local (private)"));
    }
    if ip.is_multicast() {
        return Err(ssrf("IPv6 multicast"));
    }
    Ok(())
}

/// Check whether `hostname` matches any of the allowed domains.
///
/// Rules:
/// - Exact match: `example.com` in list matches `example.com`
/// - Subdomain match: `sub.example.com` matches allowed `example.com` or `.example.com`
/// - Evil suffix bypass: `example.com.evil.com` does NOT match `example.com`
/// - Case-insensitive comparison
fn is_hostname_allowed(hostname: &str, allowed: &[String]) -> bool {
    let host_lower = hostname.trim().to_lowercase();
    if host_lower.is_empty() {
        return false;
    }
    // Strip trailing dot (FQDN notation)
    let host_lower = host_lower.strip_suffix('.').unwrap_or(&host_lower);
    // Must not be an IP address literal (already checked by SSRF, but defense in depth)
    if host_lower.parse::<std::net::Ipv4Addr>().is_ok()
        || host_lower.parse::<std::net::Ipv6Addr>().is_ok()
    {
        return false;
    }
    for allowed_domain in allowed {
        let allowed_lower = allowed_domain.trim().trim_start_matches('.').to_lowercase();
        if allowed_lower.is_empty() {
            continue;
        }
        // Exact match
        if host_lower == allowed_lower {
            return true;
        }
        // Subdomain match: host ends with ".allowed_lower"
        let suffix = format!(".{allowed_lower}");
        if host_lower.ends_with(&suffix) {
            return true;
        }
    }
    false
}

fn ssrf(reason: &str) -> WebIngestionError {
    WebIngestionError::SsrfRejected {
        url: String::new(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssrf_blocks_loopback() {
        assert!(validate_ipv4(Ipv4Addr::new(127, 0, 0, 1)).is_err());
    }
    #[test]
    fn test_ssrf_blocks_private_10() {
        assert!(validate_ipv4(Ipv4Addr::new(10, 0, 0, 1)).is_err());
    }
    #[test]
    fn test_ssrf_blocks_private_172() {
        assert!(validate_ipv4(Ipv4Addr::new(172, 16, 0, 1)).is_err());
        assert!(validate_ipv4(Ipv4Addr::new(172, 31, 255, 255)).is_err());
    }
    #[test]
    fn test_ssrf_blocks_private_192() {
        assert!(validate_ipv4(Ipv4Addr::new(192, 168, 0, 1)).is_err());
    }
    #[test]
    fn test_ssrf_blocks_metadata() {
        assert!(validate_ipv4(Ipv4Addr::new(169, 254, 169, 254)).is_err());
    }
    #[test]
    fn test_ssrf_blocks_multicast() {
        assert!(validate_ipv4(Ipv4Addr::new(224, 0, 0, 1)).is_err());
    }
    #[test]
    fn test_ssrf_allows_public() {
        assert!(validate_ipv4(Ipv4Addr::new(1, 1, 1, 1)).is_ok());
        assert!(validate_ipv4(Ipv4Addr::new(8, 8, 8, 8)).is_ok());
        assert!(validate_ipv4(Ipv4Addr::new(93, 184, 216, 34)).is_ok());
    }
    #[test]
    fn test_ssrf_allows_non_private_172() {
        assert!(validate_ipv4(Ipv4Addr::new(172, 32, 0, 1)).is_ok());
    }
    #[test]
    fn test_ssrf_blocks_ipv6_loopback() {
        assert!(validate_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)).is_err());
    }
    #[test]
    fn test_ssrf_blocks_ipv6_link_local() {
        assert!(validate_ip(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1))).is_err());
    }
    #[test]
    fn test_parse_url_parts_http() {
        let (scheme, host, port) = parse_url_parts("http://example.com/path").unwrap();
        assert_eq!(scheme, "http");
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
    }
    #[test]
    fn test_parse_url_parts_https() {
        let (scheme, host, port) = parse_url_parts("https://example.com:8443/path").unwrap();
        assert_eq!(scheme, "https");
        assert_eq!(host, "example.com");
        assert_eq!(port, 8443);
    }

    #[test]
    fn test_parse_url_parts_ipv6() {
        let (scheme, host, port) =
            parse_url_parts("https://[2606:4700:4700::1111]:8443/path").unwrap();
        assert_eq!(scheme, "https");
        assert_eq!(host, "2606:4700:4700::1111");
        assert_eq!(port, 8443);
    }

    #[test]
    fn test_parse_url_parts_rejects_invalid_port() {
        assert!(parse_url_parts("https://example.com:not-a-port/path").is_err());
    }

    #[tokio::test]
    async fn test_fetch_url_rejects_userinfo() {
        let error = validate_fetch_url("https://user:secret@example.com/path", None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("userinfo"));
    }

    #[tokio::test]
    async fn test_fetch_url_applies_allowlist_before_dns() {
        let allowed = vec!["example.com".to_string()];
        let error = validate_fetch_url("https://example.com.evil.invalid/path", Some(&allowed))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not in allowed_domains"));
    }

    #[test]
    fn test_followable_redirect_statuses() {
        for status in [301, 302, 303, 307, 308] {
            assert!(is_followable_redirect(
                reqwest::StatusCode::from_u16(status).unwrap()
            ));
        }
        for status in [200, 300, 304, 400] {
            assert!(!is_followable_redirect(
                reqwest::StatusCode::from_u16(status).unwrap()
            ));
        }
    }

    #[test]
    fn jitter_is_bounded_and_can_be_disabled() {
        assert_eq!(jitter_ms(123, 0), 0);
        assert!(jitter_ms(u64::MAX, 1_000) <= 1_000);
    }

    #[test]
    fn retry_after_parses_seconds() {
        let value = reqwest::header::HeaderValue::from_static("120");
        assert_eq!(parse_retry_after(Some(&value)), Some(120));
    }

    #[test]
    fn retry_after_rejects_invalid_value() {
        let value = reqwest::header::HeaderValue::from_static("later");
        assert_eq!(parse_retry_after(Some(&value)), None);
    }

    #[test]
    fn fetcher_accepts_http_proxy_configuration() {
        let config = WebIngestionConfig {
            fetch_proxy_url: "http://127.0.0.1:7890".into(),
            ..WebIngestionConfig::default()
        };
        assert!(WebFetcher::new(&config).is_ok());
    }

    // allowed_domains tests

    #[test]
    fn test_allowed_domains_exact_match() {
        let allowed: Vec<String> = vec!["example.com".into()];
        assert!(is_hostname_allowed("example.com", &allowed));
    }

    #[test]
    fn test_allowed_domains_subdomain() {
        let allowed: Vec<String> = vec!["example.com".into()];
        assert!(is_hostname_allowed("sub.example.com", &allowed));
        assert!(is_hostname_allowed("deep.sub.example.com", &allowed));
    }

    #[test]
    fn test_allowed_domains_evil_suffix_bypass() {
        let allowed: Vec<String> = vec!["example.com".into()];
        // evil-example.com should NOT match example.com
        assert!(!is_hostname_allowed("evil-example.com", &allowed));
        // example.com.evil.com should NOT match example.com
        assert!(!is_hostname_allowed("example.com.evil.com", &allowed));
    }

    #[test]
    fn test_allowed_domains_not_in_list() {
        let allowed: Vec<String> = vec!["safe.com".into(), "trusted.org".into()];
        assert!(!is_hostname_allowed("attacker.com", &allowed));
    }

    #[test]
    fn test_allowed_domains_case_insensitive() {
        let allowed: Vec<String> = vec!["Example.COM".into()];
        assert!(is_hostname_allowed("EXAMPLE.com", &allowed));
        assert!(is_hostname_allowed("sub.Example.Com", &allowed));
    }

    #[test]
    fn test_allowed_domains_empty_list_allows_none() {
        // Empty list: is_hostname_allowed returns false (caller should treat
        // empty list as "no restriction")
        assert!(!is_hostname_allowed("example.com", &[]));
    }

    #[test]
    fn test_allowed_domains_ip_address_rejected() {
        let allowed: Vec<String> = vec!["1.2.3.4".into()];
        assert!(!is_hostname_allowed("1.2.3.4", &allowed));
    }
}
