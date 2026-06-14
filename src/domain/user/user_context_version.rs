use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::shared::error::AppError;

#[derive(Debug, Clone)]
pub struct UserContextVersion {
    pub user_id: u64,
    pub version: u64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy)]
pub enum ContextVersionReason {
    RollingSummary,
    MemoryChanged,
    PersonaChanged,
    TranscriptCleared,
    Forget,
    PersonalizationReset,
}

#[async_trait]
pub trait UserContextVersionRepository: Send + Sync {
    async fn get_or_create(&self, user_id: u64) -> Result<UserContextVersion, AppError>;
    async fn bump(&self, user_id: u64, reason: ContextVersionReason) -> Result<u64, AppError>;
}
