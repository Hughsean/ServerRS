use crate::shared::error::AppError;

/// 统一令牌接口 — 签发和验证访问令牌与刷新令牌。
pub trait TokenServiceT: Send + Sync {
    // ── Access token ──
    fn issue_access(&self, user_id: u64, username: &str, role: &str) -> Result<String, AppError>;
    fn verify_access(&self, token: &str) -> Result<AccessTokenClaims, AppError>;

    // ── Refresh token ──
    fn issue_refresh(&self, user_id: u64, username: &str) -> Result<String, AppError>;
    fn verify_refresh(&self, refresh_token: &str) -> Result<RefreshTokenClaims, AppError>;

    // ── 第三方签名（使用调用方提供的 appKey 做 HMAC-SHA256）──
    /// 使用 appKey 作为 HMAC 密钥签发 JWT，包含 appId/iat/exp 声明。
    fn create_signature(
        &self,
        app_id: &str,
        app_key: &str,
        expires_in_seconds: i64,
    ) -> Result<String, AppError>;

    /// 使用 appKey 验证签名 JWT，返回解析出的声明。
    fn verify_signature(&self, token: &str, app_key: &str) -> Result<SignatureClaims, AppError>;
}

#[derive(Debug, Clone)]
pub struct AccessTokenClaims {
    pub user_id: u64,
    pub username: String,
    pub role: String,
}

#[derive(Debug, Clone)]
pub struct RefreshTokenClaims {
    pub user_id: u64,
    pub username: String,
    pub token_id: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone)]
pub struct SignatureClaims {
    pub valid: bool,
    pub app_id: Option<String>,
    pub issued_at: Option<i64>,
    pub expires_at: Option<i64>,
}
