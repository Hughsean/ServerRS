use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct LoginRequest {
    #[validate(length(min = 3, max = 32))]
    pub username: String,
    #[validate(length(min = 8, max = 128))]
    pub password: String,
    #[validate(length(max = 64))]
    #[serde(default)]
    pub device_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub user_id: u64,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct RegisterRequest {
    #[validate(length(min = 3, max = 32), regex(path = "*USERNAME_RE"))]
    pub username: String,
    #[validate(length(min = 8, max = 128))]
    pub password: String,
    #[validate(length(max = 64))]
    #[serde(default)]
    pub device_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub user_id: u64,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct RefreshTokenRequest {
    #[validate(length(min = 32, max = 4096))]
    pub refresh_token: String,
    #[validate(length(max = 64))]
    #[serde(default)]
    pub device_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RefreshTokenResponse {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct LogoutRequest {
    #[validate(length(min = 32, max = 4096))]
    pub refresh_token: String,
    #[validate(length(max = 128))]
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LogoutResponse {
    pub success: bool,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

static USERNAME_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^[a-zA-Z0-9_]+$").expect("valid username regex")
});
