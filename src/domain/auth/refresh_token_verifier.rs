use crate::shared::error::AppError;

#[derive(Debug, Clone)]
pub struct RefreshTokenClaims {
    pub user_id: u64,
    pub username: String,
    pub token_id: String,
    pub expires_at: u64,
}

pub trait RefreshTokenVerifier: Send + Sync {
    fn verify_refresh(&self, refresh_token: &str) -> Result<RefreshTokenClaims, AppError>;
}
