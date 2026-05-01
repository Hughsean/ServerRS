use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::domain::auth::refresh_token_revocation_repository::RefreshTokenRevocationRepository;
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
            .map(|duration| duration.as_secs())
            .map_err(|err| AppError::internal(format!("system clock error: {err}")))
    }
}

#[async_trait]
impl RefreshTokenRevocationRepository for InMemoryRefreshTokenRevocationRepository {
    async fn revoke(&self, token_id: String, expires_at: u64) -> Result<(), AppError> {
        let mut revoked = self.revoked.write().await;
        revoked.insert(token_id, expires_at);
        Ok(())
    }

    async fn is_revoked(&self, token_id: &str) -> Result<bool, AppError> {
        let now = Self::now_seconds()?;

        {
            let revoked = self.revoked.read().await;
            if let Some(expires_at) = revoked.get(token_id) {
                if *expires_at >= now {
                    return Ok(true);
                }
            } else {
                return Ok(false);
            }
        }

        // Lazy cleanup for expired records.
        let mut revoked = self.revoked.write().await;
        if let Some(expires_at) = revoked.get(token_id)
            && *expires_at < now
        {
            revoked.remove(token_id);
            return Ok(false);
        }

        Ok(revoked.contains_key(token_id))
    }

    async fn cleanup_expired(&self, now_seconds: u64) -> Result<usize, AppError> {
        let mut revoked = self.revoked.write().await;
        let before = revoked.len();
        revoked.retain(|_, expires_at| *expires_at >= now_seconds);
        Ok(before.saturating_sub(revoked.len()))
    }
}
