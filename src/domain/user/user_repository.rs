use async_trait::async_trait;

use super::user::{NewUser, User, UserUpdate};
use crate::shared::error::AppError;

#[async_trait]
pub trait UserRepoT: Send + Sync {
    async fn find_by_id(&self, id: u64) -> Result<Option<User>, AppError>;
    async fn find_by_username(&self, username: &str) -> Result<Option<User>, AppError>;
    async fn find_by_email(&self, email: &str) -> Result<Option<User>, AppError>;
    async fn find_by_phone(&self, phone: &str) -> Result<Option<User>, AppError>;
    async fn find_all(&self) -> Result<Vec<User>, AppError>;
    async fn save(&self, new_user: NewUser) -> Result<User, AppError>;
    async fn update(&self, id: u64, update: UserUpdate) -> Result<User, AppError>;
    async fn delete_by_id(&self, id: u64) -> Result<bool, AppError>;
    async fn update_last_login(&self, id: u64) -> Result<(), AppError>;

    // ── Statistics ──
    async fn count_all(&self) -> Result<u64, AppError>;
    async fn count_trend(&self, days: u32) -> Result<Vec<(String, u64)>, AppError>;
}
