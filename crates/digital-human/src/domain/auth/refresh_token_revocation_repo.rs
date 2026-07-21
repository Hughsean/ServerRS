use async_trait::async_trait;

use crate::shared::error::AppError;

#[async_trait]
pub trait RefreshTokenRevocationRepoT: Send + Sync {
    async fn revoke(&self, token_id: String, expires_at: u64) -> Result<(), AppError>;
    async fn is_revoked(&self, token_id: &str) -> Result<bool, AppError>;
    async fn cleanup_expired(&self, now_seconds: u64) -> Result<usize, AppError>;
}
