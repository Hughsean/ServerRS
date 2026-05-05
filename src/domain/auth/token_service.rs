use crate::shared::error::AppError;

/// Unified token trait — issues and verifies both access and refresh tokens.
pub trait TokenService: Send + Sync {
    // ── Access token ──
    fn issue_access(&self, user_id: u64, username: &str) -> Result<String, AppError>;
    fn verify_access(&self, token: &str) -> Result<AccessTokenClaims, AppError>;

    // ── Refresh token ──
    fn issue_refresh(&self, user_id: u64, username: &str) -> Result<String, AppError>;
    fn verify_refresh(&self, refresh_token: &str) -> Result<RefreshTokenClaims, AppError>;
}

#[derive(Debug, Clone)]
pub struct AccessTokenClaims {
    pub user_id: u64,
    pub username: String,
}

#[derive(Debug, Clone)]
pub struct RefreshTokenClaims {
    pub user_id: u64,
    pub username: String,
    pub token_id: String,
    pub expires_at: u64,
}
