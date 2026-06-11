use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::shared::error::AppError;

#[derive(Debug, Clone)]
pub struct ContentLike {
    pub like_id: u64,
    pub user_id: u64,
    pub content_type: String,
    pub content_id: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewContentLike {
    pub user_id: u64,
    pub content_type: String,
    pub content_id: u64,
}

#[async_trait]
pub trait ContentLikeRepository: Send + Sync {
    /// Toggle a like for the given user and content.
    /// Returns `true` if the content is now liked, `false` if it has been unliked.
    async fn toggle(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError>;

    /// Check whether the given user has liked the given content.
    async fn is_liked(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError>;

    /// Return the total number of likes for the given content.
    async fn count_by_content(&self, content_type: &str, content_id: u64) -> Result<u64, AppError>;

    /// Delete a specific like record.
    /// Returns `true` if a record was actually deleted, `false` if none existed.
    async fn delete(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError>;
}
