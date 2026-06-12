//! SSRF-safe HTTP fetcher for web ingestion.
//!
//! Guards: only http/https, blocks private/localhost/link-local/multicast IPs,
//! blocks metadata IPs (169.254.169.254), Content-Type allowlist, body size limit,
//! no automatic redirect following.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use reqwest::header::CONTENT_TYPE;

use crate::domain::web_ingestion::error::WebIngestionError;
use crate::shared::config::WebIngestionConfig;

const ALLOWED_CONTENT_TYPES: &[&str] = &[
    "text/html",
    "text/plain",
    "application/xhtml+xml",
    "application/xml",
];

#[derive(Debug, Clone)]
pub struct FetchResult {
    pub final_url: String,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub body_text: String,
    pub content_length: Option<u64>,
}

pub struct WebFetcher {
    client: reqwest::Client,
    max_body_bytes: u64,
}

impl WebFetcher {
    pub fn new(config: &WebIngestionConfig) -> Result<Self, WebIngestionError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(config.fetch_timeout_secs))
            .build()
            .map_err(|e| {
                WebIngestionError::Internal(format!("failed to build HTTP client: {e}"))
            })?;
        Ok(Self {
            client,
            max_body_bytes: config.max_body_bytes,
        })
    }

    /// Fetch a URL with SSRF protection and optional domain allowlist.
    ///
    /// `allowed_domains` — when non-empty, the hostname must match one of the
    /// allowed domains (exact or subdomain).  Domain matching uses suffix-match:
    /// `sub.example.com` matches allowed `.example.com` or `example.com`.
    pub async fn fetch(
        &self,
        url: &str,
        allowed_domains: Option<&[String]>,
    ) -> Result<FetchResult, WebIngestionError> {
        // 1. Parse URL — simple parsing to extract host and scheme
        let (scheme, host, _port) = parse_url_parts(url)?;

        // 2. Scheme check
        if scheme != "http" && scheme != "https" {
            return Err(WebIngestionError::SsrfRejected {
                url: url.to_string(),
                reason: format!("scheme not allowed: {scheme}"),
            });
        }

        // 3. Validate hostname against allowed_domains (if configured)
        if let Some(ref allowed) = allowed_domains {
            if !allowed.is_empty() {
                if !is_hostname_allowed(&host, allowed) {
                    return Err(WebIngestionError::SsrfRejected {
                        url: url.to_string(),
                        reason: format!(
                            "hostname '{host}' not in allowed_domains: {}",
                            allowed.join(", ")
                        ),
                    });
                }
            }
        }

        // 4. Validate no userinfo / fragment bypasses
        if host.is_empty() || host.starts_with('[') || host.contains('@') {
            return Err(WebIngestionError::SsrfRejected {
                url: url.to_string(),
                reason: format!("suspicious hostname: {host}"),
            });
        }

        // 5. Resolve host and validate IP
        let port = if scheme == "https" { 443 } else { 80 };
        let addr_str = format!("{host}:{port}");
        let ips = tokio::net::lookup_host(&addr_str).await.map_err(|e| {
            WebIngestionError::SsrfRejected {
                url: url.to_string(),
                reason: format!("DNS resolution failed: {e}"),
            }
        })?;

        for addr in ips {
            validate_ip(addr.ip()).map_err(|e| {
                if let WebIngestionError::SsrfRejected { reason, .. } = e {
                    WebIngestionError::SsrfRejected {
                        url: url.to_string(),
                        reason,
                    }
                } else {
                    e
                }
            })?;
        }

        // 6. Fetch
        let response =
            self.client
                .get(url)
                .send()
                .await
                .map_err(|e| WebIngestionError::FetchFailed {
                    url: url.to_string(),
                    reason: e.to_string(),
                })?;

        let final_url = response.url().to_string();
        let status = response.status();
        if !status.is_success() {
            return Err(WebIngestionError::FetchFailed {
                url: final_url,
                reason: format!("HTTP {status}"),
            });
        }

        // 5. Content-Type check
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

        // 6. Read body with size limit
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
}

/// Minimal URL parsing to extract scheme, host, and port.
/// Avoids adding the `url` crate dependency.
fn parse_url_parts(raw: &str) -> Result<(String, String, u16), WebIngestionError> {
    let url = raw.trim();
    // Find scheme
    let (scheme, rest) = if let Some(idx) = url.find("://") {
        (url[..idx].to_lowercase(), &url[idx + 3..])
    } else {
        return Err(WebIngestionError::SsrfRejected {
            url: raw.to_string(),
            reason: "no scheme in URL".into(),
        });
    };

    // Find host (stop at first /, :, ?, or #)
    let host_end = rest
        .find(|c: char| c == '/' || c == ':' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let host = rest[..host_end].to_lowercase();

    if host.is_empty() {
        return Err(WebIngestionError::SsrfRejected {
            url: raw.to_string(),
            reason: "no host in URL".into(),
        });
    }

    // Extract port if present
    let port = if host_end < rest.len() && rest.as_bytes()[host_end] == b':' {
        let port_start = host_end + 1;
        let port_end = rest[port_start..]
            .find(|c: char| c == '/' || c == '?' || c == '#')
            .map(|p| port_start + p)
            .unwrap_or(rest.len());
        rest[port_start..port_end]
            .parse::<u16>()
            .unwrap_or(if scheme == "https" { 443 } else { 80 })
    } else if scheme == "https" {
        443
    } else {
        80
    };

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

    // ── allowed_domains tests ──────────────────────────────────────────────

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
