use async_trait::async_trait;

use crate::shared::error::AppError;

#[async_trait]
pub trait RefreshTokenStore: Send + Sync {
    async fn store(&self, user_id: u64, token_hash: String) -> Result<(), AppError>;
    async fn is_revoked(&self, token_hash: &str) -> Result<bool, AppError>;
    async fn revoke(&self, token_hash: &str) -> Result<(), AppError>;
    async fn cleanup_expired(&self, now_seconds: u64) -> Result<usize, AppError>;
}
