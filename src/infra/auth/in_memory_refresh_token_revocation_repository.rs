use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::domain::auth::refresh_token_revocation_repository::RefreshTokenRevocationRepository;
use crate::domain::auth::refresh_token_store::RefreshTokenStoreT;
use crate::shared::error::AppError;

#[derive(Default)]
pub struct InMemoryRefreshTokenRevocationRepository {
    revoked: Arc<RwLock<HashMap<String, u64>>>,
}

impl InMemoryRefreshTokenRevocationRepository {
    pub fn new() -> Self {
        Self::default()
    }

    fn now_seconds() -> Result<u64, AppError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|e| AppError::internal(format!("clock error: {e}")))
    }
}

#[async_trait]
impl RefreshTokenRevocationRepository for InMemoryRefreshTokenRevocationRepository {
    async fn revoke(&self, token_id: String, expires_at: u64) -> Result<(), AppError> {
        self.revoked.write().await.insert(token_id, expires_at);
        Ok(())
    }

    async fn is_revoked(&self, token_id: &str) -> Result<bool, AppError> {
        let now = Self::now_seconds()?;
        let revoked = self.revoked.read().await;
        Ok(revoked.get(token_id).map_or(false, |&exp| exp >= now))
    }

    async fn cleanup_expired(&self, now_seconds: u64) -> Result<usize, AppError> {
        let mut revoked = self.revoked.write().await;
        let before = revoked.len();
        revoked.retain(|_, exp| *exp >= now_seconds);
        Ok(before.saturating_sub(revoked.len()))
    }
}

// 为新 AuthService 提供的 RefreshTokenStore 适配器
#[async_trait]
impl RefreshTokenStoreT for InMemoryRefreshTokenRevocationRepository {
    async fn store(&self, _user_id: u64, token_hash: String) -> Result<(), AppError> {
        // 内存模式：无需持久化；Token 在被撤销前始终有效
        let _ = token_hash;
        Ok(())
    }

    async fn is_revoked(&self, token_hash: &str) -> Result<bool, AppError> {
        let now = Self::now_seconds()?;
        let revoked = self.revoked.read().await;
        Ok(revoked.get(token_hash).map_or(false, |&exp| exp >= now))
    }

    async fn revoke(&self, token_hash: &str) -> Result<(), AppError> {
        // Store with far-future expiry so is_revoked returns true
        let far_future = u64::MAX / 2;
        self.revoked
            .write()
            .await
            .insert(token_hash.to_string(), far_future);
        Ok(())
    }

    async fn cleanup_expired(&self, _now_seconds: u64) -> Result<usize, AppError> {
        Ok(0)
    }
}
