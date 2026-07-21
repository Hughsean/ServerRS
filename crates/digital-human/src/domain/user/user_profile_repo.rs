use async_trait::async_trait;

use super::user_profile::{NewUserProfile, UserProfile, UserProfileUpdate};
use crate::shared::error::AppError;

#[async_trait]
pub trait UserProfileRepoT: Send + Sync {
    async fn find_by_user_id(&self, user_id: u64) -> Result<Option<UserProfile>, AppError>;
    async fn save(&self, profile: NewUserProfile) -> Result<UserProfile, AppError>;
    async fn update(
        &self,
        user_id: u64,
        update: UserProfileUpdate,
    ) -> Result<UserProfile, AppError>;

    /// 原子化创建或更新用户画像。
    /// 使用 INSERT ... ON DUPLICATE KEY UPDATE 语义，
    /// 消除 find-then-write 的 TOCTOU 竞态条件。
    async fn upsert(&self, user_id: u64, profile: NewUserProfile) -> Result<UserProfile, AppError>;

    async fn delete_by_user_id(&self, user_id: u64) -> Result<bool, AppError>;
}
