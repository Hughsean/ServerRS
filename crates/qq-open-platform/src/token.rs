use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{QqApiError, QqBotCredentials, QqOpenPlatformEndpoints};

const DEFAULT_EXPIRES_SECS: u64 = 7200;
const REFRESH_AHEAD: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenStatus {
    Empty,
    Valid,
    RefreshRequired,
}

struct CachedToken {
    value: String,
    expires_at: Instant,
}

pub(crate) struct TokenManager {
    client: Client,
    endpoints: QqOpenPlatformEndpoints,
    credentials: QqBotCredentials,
    state: Mutex<Option<CachedToken>>,
}

impl TokenManager {
    pub(crate) fn new(
        client: Client,
        endpoints: QqOpenPlatformEndpoints,
        credentials: QqBotCredentials,
    ) -> Self {
        Self {
            client,
            endpoints,
            credentials,
            state: Mutex::new(None),
        }
    }

    pub(crate) async fn access_token(&self) -> Result<String, QqApiError> {
        // Mutex 跨换取请求持有，形成单账号 singleflight；不同账号实例互不阻塞。
        let mut state = self.state.lock().await;
        if let Some(cached) = state.as_ref() {
            let remaining = cached.expires_at.saturating_duration_since(Instant::now());
            let refresh_ahead = REFRESH_AHEAD.min(remaining / 3);
            if remaining > refresh_ahead {
                return Ok(cached.value.clone());
            }
        }
        let response = self
            .client
            .post(self.endpoints.token_url.clone())
            .json(&serde_json::json!({
                "appId": self.credentials.app_id(),
                "clientSecret": self.credentials.client_secret(),
            }))
            .send()
            .await
            .map_err(QqApiError::transport)?;
        let status = response.status();
        let body: Value = response.json().await.map_err(QqApiError::transport)?;
        if !status.is_success() {
            return Err(QqApiError::Rejected {
                status: status.as_u16(),
                code: body.get("code").and_then(Value::as_i64),
            });
        }
        let token = body
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty() && value.len() <= 8_192)
            .ok_or_else(|| QqApiError::Protocol("token response has no access_token".into()))?;
        let expires_secs = parse_expires(body.get("expires_in")).unwrap_or(DEFAULT_EXPIRES_SECS);
        *state = Some(CachedToken {
            value: token.to_owned(),
            expires_at: Instant::now() + Duration::from_secs(expires_secs),
        });
        tracing::debug!(
            app_id = self.credentials.app_id(),
            expires_secs,
            "QQ Open Platform access token refreshed"
        );
        Ok(token.to_owned())
    }

    pub(crate) async fn clear(&self) {
        *self.state.lock().await = None;
    }

    pub(crate) async fn status(&self) -> TokenStatus {
        match self.state.lock().await.as_ref() {
            None => TokenStatus::Empty,
            Some(cached) if cached.expires_at > Instant::now() + Duration::from_secs(1) => {
                TokenStatus::Valid
            }
            Some(_) => TokenStatus::RefreshRequired,
        }
    }
}

fn parse_expires(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64().filter(|value| (1..=86_400).contains(value)),
        Value::String(value) => value
            .parse()
            .ok()
            .filter(|value| (1..=86_400).contains(value)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_expiry_is_bounded_before_instant_arithmetic() {
        assert_eq!(parse_expires(Some(&Value::from(7_200))), Some(7_200));
        assert_eq!(parse_expires(Some(&Value::from("3600"))), Some(3_600));
        assert_eq!(parse_expires(Some(&Value::from(0))), None);
        assert_eq!(parse_expires(Some(&Value::from(u64::MAX))), None);
    }
}
