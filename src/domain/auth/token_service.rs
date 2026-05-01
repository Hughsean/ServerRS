use crate::shared::error::AppError;

/// Unified access token trait — replaces TokenIssuer + TokenVerifier.
pub trait TokenService: Send + Sync {
    fn issue(&self, user_id: u64, username: &str) -> Result<String, AppError>;
    fn verify(&self, token: &str) -> Result<AccessTokenClaims, AppError>;
}

#[derive(Debug, Clone)]
pub struct AccessTokenClaims {
    pub user_id: u64,
    pub username: String,
}
