use crate::shared::error::AppError;

#[derive(Debug, Clone)]
pub struct AccessTokenClaims {
    pub user_id: u64,
    pub username: String,
}

pub trait TokenVerifier: Send + Sync {
    fn verify(&self, token: &str) -> Result<AccessTokenClaims, AppError>;
}
