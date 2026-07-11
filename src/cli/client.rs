//! HTTP 客户端:封装后端 API 调用与 401 自动刷新重试。
//!
//! 设计:用 `HttpBackend` trait 抽象底层 HTTP,生产用 reqwest,
//! 测试用 mock 实现,避免引入 wiremock 依赖。
//! 401 处理统一在 `request` 内:刷新 token 后重试一次,仍 401 则返回
//! `Auth("需要重新登录")`,由上层(REPL)触发交互登录。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use reqwest::{Client, Method, Response, StatusCode};

use crate::cli::dto::*;
use crate::cli::error::CliError;
use crate::cli::{auth::TokenCache, config::CliConfig};

/// 底层 HTTP 抽象,便于测试 mock。
#[async_trait]
pub trait HttpBackend: Send + Sync {
    /// 执行请求,返回 (status, body_text)。
    /// `auth_header` 为 None 时不带 Authorization。
    async fn execute(
        &self,
        method: Method,
        url: String,
        auth_header: Option<String>,
        body: Option<String>,
    ) -> Result<(StatusCode, String), CliError>;
}

/// 生产实现:基于 reqwest。
pub struct ReqwestBackend {
    client: Client,
}

impl ReqwestBackend {
    pub fn new() -> Result<Self, CliError> {
        Ok(Self {
            client: Client::builder().build()?,
        })
    }
}

#[async_trait]
impl HttpBackend for ReqwestBackend {
    async fn execute(
        &self,
        method: Method,
        url: String,
        auth_header: Option<String>,
        body: Option<String>,
    ) -> Result<(StatusCode, String), CliError> {
        let mut req = self.client.request(method, &url);
        if let Some(h) = auth_header {
            req = req.header("Authorization", h);
        }
        if let Some(b) = body {
            req = req.header("Content-Type", "application/json").body(b);
        }
        let resp: Response = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        Ok((status, text))
    }
}

/// API 客户端。token 用 Mutex 保护,因 401 刷新时会修改。
pub struct ApiClient {
    base_url: String,
    backend: Arc<dyn HttpBackend>,
    token: Mutex<Option<TokenCache>>,
}

impl ApiClient {
    pub fn new(
        config: &CliConfig,
        backend: Arc<dyn HttpBackend>,
        token: Option<TokenCache>,
    ) -> Self {
        Self {
            base_url: config.base_url.clone(),
            backend,
            token: Mutex::new(token),
        }
    }

    /// 生产构造:用 reqwest backend。
    pub fn with_reqwest(config: &CliConfig, token: Option<TokenCache>) -> Result<Self, CliError> {
        Ok(Self::new(config, Arc::new(ReqwestBackend::new()?), token))
    }

    fn current_access(&self) -> Option<String> {
        self.token.lock().ok()?.as_ref().map(|t| t.access_token.clone())
    }

    fn current_refresh(&self) -> Option<String> {
        self.token
            .lock()
            .ok()?
            .as_ref()
            .map(|t| t.refresh_token.clone())
    }

    fn set_token(&self, cache: TokenCache) {
        if let Ok(mut t) = self.token.lock() {
            *t = Some(cache);
        }
    }

    /// 供启动流程注入缓存 token。
    pub fn set_token_external(&self, token: TokenCache) {
        self.set_token(token);
    }

    /// 统一请求入口。401 时尝试 refresh 后重试一次。
    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
        auth: bool,
    ) -> Result<T, CliError> {
        let url = format!("{}{}", self.base_url, path);
        let auth_header = if auth {
            self.current_access().map(|t| format!("Bearer {t}"))
        } else {
            None
        };
        let (status, text) = self
            .backend
            .execute(method.clone(), url.clone(), auth_header, body.clone())
            .await?;

        if status == StatusCode::UNAUTHORIZED && auth {
            // 尝试 refresh
            if self.try_refresh().await? {
                let auth_header = self.current_access().map(|t| format!("Bearer {t}"));
                let (status2, text2) = self
                    .backend
                    .execute(method, url, auth_header, body)
                    .await?;
                return Self::parse(status2, text2);
            }
            return Err(CliError::Auth("登录已过期,需要重新登录".into()));
        }
        Self::parse(status, text)
    }

    /// 解析状态码与响应体。
    fn parse<T: serde::de::DeserializeOwned>(
        status: StatusCode,
        text: String,
    ) -> Result<T, CliError> {
        if status.is_success() {
            return serde_json::from_str(&text).map_err(CliError::Serde);
        }
        // 尝试提取后端错误消息
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .or_else(|| v.get("message"))
                    .and_then(|m| m.as_str())
                    .map(String::from)
            })
            .unwrap_or(text);
        Err(CliError::Api { status: status.as_u16(), msg })
    }

    /// 用 refresh_token 刷新,成功则更新内存 token 并返回 true。
    /// refresh 失败返回 false(不返回 Err,让上层走重新登录)。
    async fn try_refresh(&self) -> Result<bool, CliError> {
        let Some(refresh_token) = self.current_refresh() else {
            return Ok(false);
        };
        let url = format!("{}/api/v1/auth/refresh", self.base_url);
        let body = serde_json::json!({ "refresh_token": refresh_token }).to_string();
        let (status, text) = self
            .backend
            .execute(Method::POST, url, None, Some(body))
            .await?;
        if status.is_success() {
            let resp: RefreshResponse = serde_json::from_str(&text)?;
            self.set_token(resp.into());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // ── 公开 API 方法 ──

    pub async fn login(&self, username: &str, password: &str) -> Result<TokenCache, CliError> {
        let body = serde_json::json!({ "username": username, "password": password }).to_string();
        let url = format!("{}/api/v1/auth/login", self.base_url);
        let (status, text) = self
            .backend
            .execute(Method::POST, url, None, Some(body))
            .await?;
        if !status.is_success() {
            let msg = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("error").and_then(|m| m.as_str()).map(String::from))
                .unwrap_or_else(|| "用户名或密码错误".into());
            return Err(CliError::Auth(msg));
        }
        let resp: LoginResponse = serde_json::from_str(&text)?;
        let cache: TokenCache = resp.into();
        self.set_token(cache.clone());
        Ok(cache)
    }

    pub async fn me(&self) -> Result<AuthUser, CliError> {
        self.request(Method::GET, "/api/v1/auth/me", None, true)
            .await
    }

    pub async fn chat_open(&self) -> Result<ChatOpenResponse, CliError> {
        self.request(Method::POST, "/api/v1/chat/open", Some("{}".into()), true)
            .await
    }

    pub async fn chat_send(&self, text: &str) -> Result<ChatMessageResponse, CliError> {
        let body = serde_json::to_string(&ChatMessageRequest { text: text.into() })?;
        self.request(Method::POST, "/api/v1/chat/messages", Some(body), true)
            .await
    }

    pub async fn chat_history(&self, limit: u64) -> Result<ChatHistoryResponse, CliError> {
        self.request(
            Method::GET,
            &format!("/api/v1/chat/history?limit={limit}"),
            None,
            true,
        )
        .await
    }

    pub async fn chat_memories(
        &self,
        mem_type: Option<&str>,
        limit: usize,
    ) -> Result<ChatMemoryResponse, CliError> {
        let mut path = format!("/api/v1/chat/memories?limit={limit}");
        if let Some(t) = mem_type {
            path.push_str(&format!("&type={t}"));
        }
        self.request(Method::GET, &path, None, true).await
    }

    pub async fn chat_persona(&self) -> Result<ChatPersonaResponse, CliError> {
        self.request(Method::GET, "/api/v1/chat/persona", None, true)
            .await
    }

    pub async fn user_profile(&self) -> Result<UserProfileResponse, CliError> {
        self.request(Method::GET, "/api/v1/users/me/profile", None, true)
            .await
    }

    pub async fn transcript_clear(&self) -> Result<TranscriptClearResponse, CliError> {
        self.request(Method::POST, "/api/v1/chat/transcript/clear", None, true)
            .await
    }

    pub async fn forget(&self) -> Result<ForgetResponse, CliError> {
        self.request(Method::POST, "/api/v1/chat/forget", None, true)
            .await
    }

    pub async fn persona_rebuild(&self) -> Result<PersonaRebuildResponse, CliError> {
        self.request(Method::POST, "/api/v1/chat/persona/rebuild", None, true)
            .await
    }

    pub async fn persona_reset(&self) -> Result<PersonaResetResponse, CliError> {
        self.request(Method::POST, "/api/v1/chat/persona/reset", None, true)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 可编程 mock backend:按调用次数返回预设响应。
    struct MockBackend {
        responses: Mutex<Vec<(StatusCode, String)>>,
        calls: Mutex<Vec<(String, Option<String>)>>,
        refresh_calls: AtomicUsize,
    }

    impl MockBackend {
        fn new(responses: Vec<(StatusCode, String)>) -> Self {
            Self {
                responses: Mutex::new(responses),
                calls: Mutex::new(vec![]),
                refresh_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl HttpBackend for MockBackend {
        async fn execute(
            &self,
            _method: Method,
            url: String,
            auth_header: Option<String>,
            _body: Option<String>,
        ) -> Result<(StatusCode, String), CliError> {
            let is_refresh = url.contains("/auth/refresh");
            self.calls.lock().unwrap().push((url, auth_header));
            if is_refresh {
                self.refresh_calls.fetch_add(1, Ordering::SeqCst);
            }
            let mut resp = self.responses.lock().unwrap();
            if resp.is_empty() {
                panic!("MockBackend 无更多预设响应");
            }
            Ok(resp.remove(0))
        }
    }

    fn cfg() -> CliConfig {
        CliConfig {
            base_url: "http://test".into(),
            token_path: std::path::PathBuf::from("/tmp/x"),
        }
    }

    #[tokio::test]
    async fn request_retries_after_401_with_successful_refresh() {
        // 第一次 me 返回 401,refresh 成功,重试 me 返回 200
        let mock = Arc::new(MockBackend::new(vec![
            (StatusCode::UNAUTHORIZED, r#"{"error":"expired"}"#.into()),
            (
                StatusCode::OK,
                r#"{"access_token":"new","refresh_token":"newr"}"#.into(),
            ),
            (StatusCode::OK, r#"{"id":1,"username":"alice"}"#.into()),
        ]));
        let client = ApiClient::new(
            &cfg(),
            mock.clone(),
            Some(TokenCache {
                access_token: "old".into(),
                refresh_token: "oldr".into(),
            }),
        );
        let user: AuthUser = client.me().await.unwrap();
        assert_eq!(user.username, "alice");
        assert_eq!(mock.refresh_calls.load(Ordering::SeqCst), 1);
        // 刷新后内存 token 应更新
        assert_eq!(client.current_access().unwrap(), "new");
    }

    #[tokio::test]
    async fn request_returns_relogin_when_refresh_fails() {
        // me 401,refresh 也 401 -> 返回 Auth("需要重新登录")
        let mock = Arc::new(MockBackend::new(vec![
            (StatusCode::UNAUTHORIZED, "{}".into()),
            (StatusCode::UNAUTHORIZED, "{}".into()),
        ]));
        let client = ApiClient::new(
            &cfg(),
            mock,
            Some(TokenCache {
                access_token: "old".into(),
                refresh_token: "oldr".into(),
            }),
        );
        let err = client.me().await.unwrap_err();
        assert!(err.is_relogin_required());
    }

    #[tokio::test]
    async fn login_success_sets_token() {
        let mock = Arc::new(MockBackend::new(vec![(
            StatusCode::OK,
            r#"{"user_id":1,"access_token":"a","refresh_token":"r"}"#.into(),
        )]));
        let client = ApiClient::new(&cfg(), mock, None);
        let cache = client.login("alice", "pw").await.unwrap();
        assert_eq!(cache.access_token, "a");
        assert_eq!(client.current_access().unwrap(), "a");
    }

    #[tokio::test]
    async fn login_failure_returns_auth_error() {
        let mock = Arc::new(MockBackend::new(vec![(
            StatusCode::UNAUTHORIZED,
            r#"{"error":"用户名或密码错误"}"#.into(),
        )]));
        let client = ApiClient::new(&cfg(), mock, None);
        let err = client.login("alice", "wrong").await.unwrap_err();
        match err {
            CliError::Auth(msg) => assert!(msg.contains("用户名或密码错误")),
            other => panic!("期望 Auth 错误,得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn api_error_extracts_message() {
        let mock = Arc::new(MockBackend::new(vec![(
            StatusCode::BAD_REQUEST,
            r#"{"error":"参数无效"}"#.into(),
        )]));
        let client = ApiClient::new(
            &cfg(),
            mock,
            Some(TokenCache {
                access_token: "t".into(),
                refresh_token: "r".into(),
            }),
        );
        let err: CliError = client.chat_open().await.unwrap_err();
        match err {
            CliError::Api { status, msg } => {
                assert_eq!(status, 400);
                assert!(msg.contains("参数无效"));
            }
            other => panic!("期望 Api 错误,得到 {other:?}"),
        }
    }
}
