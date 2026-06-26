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
pub trait ContentLikeRepoT: Send + Sync {
    /// 切换给定用户和内容的点赞状态。
    /// Returns `true` if the content is now liked, `false` if it has been unliked.
    async fn toggle(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError>;

    /// 检查给定用户是否已点赞给定内容。
    async fn is_liked(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError>;

    /// 返回给定内容的点赞总数。
    async fn count_by_content(&self, content_type: &str, content_id: u64) -> Result<u64, AppError>;

    /// 删除特定的点赞记录。
    /// Returns `true` if a record was actually deleted, `false` if none existed.
    async fn delete(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError>;
}
