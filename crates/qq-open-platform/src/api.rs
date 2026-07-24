use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use thiserror::Error;

use crate::token::TokenManager;
use crate::{QqBotCredentials, TokenStatus};

#[derive(Debug, Clone)]
pub struct QqOpenPlatformEndpoints {
    pub token_url: Url,
    pub api_base_url: Url,
}

impl Default for QqOpenPlatformEndpoints {
    fn default() -> Self {
        Self {
            token_url: Url::parse("https://bots.qq.com/app/getAppAccessToken").unwrap(),
            api_base_url: Url::parse("https://api.sgroup.qq.com/").unwrap(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QqTarget {
    C2c { user_openid: String },
    Group { group_openid: String },
}

impl QqTarget {
    fn path(&self) -> Result<String, QqApiError> {
        let (prefix, id) = match self {
            Self::C2c { user_openid } => ("v2/users", user_openid),
            Self::Group { group_openid } => ("v2/groups", group_openid),
        };
        if id.trim().is_empty() || id.len() > 512 || id.contains('/') {
            return Err(QqApiError::InvalidTarget);
        }
        Ok(format!("{prefix}/{id}/messages"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QqMessageReceipt {
    pub platform_message_id: String,
}

pub struct QqOpenPlatformClient {
    client: Client,
    endpoints: QqOpenPlatformEndpoints,
    credentials: QqBotCredentials,
    token: Arc<TokenManager>,
}

impl QqOpenPlatformClient {
    pub fn new(credentials: QqBotCredentials) -> Result<Self, QqApiError> {
        Self::with_endpoints(credentials, QqOpenPlatformEndpoints::default())
    }

    pub fn with_endpoints(
        credentials: QqBotCredentials,
        endpoints: QqOpenPlatformEndpoints,
    ) -> Result<Self, QqApiError> {
        validate_endpoint(&endpoints.token_url, "bots.qq.com")?;
        validate_endpoint(&endpoints.api_base_url, "api.sgroup.qq.com")?;
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .user_agent("ServerRS-QQPersonalSecretary/1.0")
            .build()
            .map_err(QqApiError::transport)?;
        let token = Arc::new(TokenManager::new(
            client.clone(),
            endpoints.clone(),
            credentials.clone(),
        ));
        Ok(Self {
            client,
            endpoints,
            credentials,
            token,
        })
    }

    pub fn app_id(&self) -> &str {
        self.credentials.app_id()
    }

    pub async fn get_gateway_url(&self) -> Result<Url, QqApiError> {
        let response: GatewayResponse = self.request_json("gateway", None).await?;
        let url = Url::parse(&response.url)
            .map_err(|error| QqApiError::Protocol(format!("invalid gateway URL: {error}")))?;
        if url.scheme() != "wss" {
            return Err(QqApiError::Protocol("QQ gateway URL must use wss".into()));
        }
        if url.host_str() != Some("api.sgroup.qq.com")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(QqApiError::Protocol(
                "QQ gateway URL has an unapproved authority".into(),
            ));
        }
        Ok(url)
    }

    pub async fn send_text(
        &self,
        target: &QqTarget,
        content: &str,
    ) -> Result<QqMessageReceipt, QqApiError> {
        if content.trim().is_empty() || content.chars().count() > 4000 {
            return Err(QqApiError::InvalidContent);
        }
        let path = target.path()?;
        let body = serde_json::json!({ "content": content, "msg_type": 0 });
        let response: MessageResponse = self.request_json(&path, Some(body)).await?;
        if response.id.trim().is_empty() || response.id.len() > 512 {
            return Err(QqApiError::Protocol(
                "message response has an invalid message id".into(),
            ));
        }
        Ok(QqMessageReceipt {
            platform_message_id: response.id,
        })
    }

    pub async fn access_token(&self) -> Result<String, QqApiError> {
        self.token.access_token().await
    }

    pub async fn clear_token(&self) {
        self.token.clear().await;
    }

    pub async fn token_status(&self) -> TokenStatus {
        self.token.status().await
    }

    async fn request_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, QqApiError> {
        let mut retried = false;
        loop {
            let token = self.token.access_token().await?;
            let url = self
                .endpoints
                .api_base_url
                .join(path)
                .map_err(|error| QqApiError::Protocol(error.to_string()))?;
            let request = if let Some(body) = &body {
                self.client.post(url).json(body)
            } else {
                self.client.get(url)
            }
            .header("Authorization", format!("QQBot {token}"));
            let response = request.send().await.map_err(QqApiError::transport)?;
            if response.status() == StatusCode::UNAUTHORIZED && !retried {
                self.token.clear().await;
                retried = true;
                continue;
            }
            let status = response.status();
            if !status.is_success() {
                let error: serde_json::Value = response.json().await.unwrap_or_default();
                return Err(match status {
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => QqApiError::Unauthorized,
                    StatusCode::TOO_MANY_REQUESTS => QqApiError::RateLimited,
                    _ => QqApiError::Rejected {
                        status: status.as_u16(),
                        code: error.get("code").and_then(serde_json::Value::as_i64),
                    },
                });
            }
            return response.json().await.map_err(QqApiError::transport);
        }
    }
}

#[derive(Debug, Deserialize)]
struct GatewayResponse {
    url: String,
}

#[derive(Debug, Deserialize)]
struct MessageResponse {
    id: String,
}

fn validate_endpoint(url: &Url, expected_host: &str) -> Result<(), QqApiError> {
    if url.scheme() != "https" {
        return Err(QqApiError::InvalidEndpoint);
    }
    // 测试可用 loopback HTTPS；生产默认严格固定官方 Host。
    let Some(host) = url.host_str() else {
        return Err(QqApiError::InvalidEndpoint);
    };
    if host != expected_host && !matches!(host, "127.0.0.1" | "localhost") {
        return Err(QqApiError::InvalidEndpoint);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum QqApiError {
    #[error("QQ Open Platform endpoint is not an approved HTTPS host")]
    InvalidEndpoint,
    #[error("QQ Open Platform target is invalid")]
    InvalidTarget,
    #[error("QQ message content must contain 1..=4000 characters")]
    InvalidContent,
    #[error("QQ Open Platform authentication failed")]
    Unauthorized,
    #[error("QQ Open Platform rate limit reached")]
    RateLimited,
    #[error("QQ Open Platform rejected the request (HTTP {status}, code {code:?})")]
    Rejected { status: u16, code: Option<i64> },
    #[error("QQ Open Platform transport failed: {0}")]
    Transport(String),
    #[error("QQ Open Platform protocol error: {0}")]
    Protocol(String),
}

impl QqApiError {
    pub(crate) fn transport(error: impl std::fmt::Display) -> Self {
        Self::Transport(error.to_string())
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited
                | Self::Transport(_)
                | Self::Rejected {
                    status: 500..=599,
                    ..
                }
        )
    }
}
