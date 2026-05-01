use crate::shared::error::AppError;

/// Unified refresh token trait — replaces RefreshTokenIssuer + RefreshTokenVerifier.
pub trait RefreshTokenService: Send + Sync {
    fn issue(&self, user_id: u64, username: &str) -> Result<String, AppError>;
    fn verify(&self, refresh_token: &str) -> Result<RefreshTokenClaims, AppError>;
}

#[derive(Debug, Clone)]
pub struct RefreshTokenClaims {
    pub user_id: u64,
    pub username: String,
    pub token_id: String,
    pub expires_at: u64,
}
