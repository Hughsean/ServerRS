use async_trait::async_trait;

use super::user_profile::{NewUserProfile, UserProfile, UserProfileUpdate};
use crate::shared::error::AppError;

#[async_trait]
pub trait UserProfileRepository: Send + Sync {
    async fn find_by_user_id(&self, user_id: u64) -> Result<Option<UserProfile>, AppError>;
    async fn save(&self, profile: NewUserProfile) -> Result<UserProfile, AppError>;
    async fn update(
        &self,
        user_id: u64,
        update: UserProfileUpdate,
    ) -> Result<UserProfile, AppError>;
    async fn delete_by_user_id(&self, user_id: u64) -> Result<bool, AppError>;
}
