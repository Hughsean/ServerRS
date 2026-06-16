use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::shared::error::AppError;

/// 匹配 user_diaries 实体。当前数据库中没有 tags 或 mood_score 列。
/// mood_description is the only optional mood field persisted.
#[derive(Debug, Clone, Serialize)]
pub struct UserDiary {
    pub id: u64,
    pub user_id: u64,
    pub title: String,
    pub content: String,
    pub mood_description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewUserDiary {
    pub user_id: u64,
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct UserDiaryUpdate {
    pub title: Option<String>,
    pub content: Option<String>,
    pub mood_description: Option<Option<String>>,
}

#[async_trait]
pub trait DiaryRepository: Send + Sync {
    async fn save(&self, diary: NewUserDiary) -> Result<UserDiary, AppError>;
    async fn find_by_id(&self, id: u64) -> Result<Option<UserDiary>, AppError>;
    async fn find_by_user_id(
        &self,
        user_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<UserDiary>, u64), AppError>;
    async fn update(&self, id: u64, update: UserDiaryUpdate) -> Result<UserDiary, AppError>;
    async fn update_mood(&self, id: u64, mood_description: String) -> Result<(), AppError>;
    async fn delete_by_id(&self, id: u64) -> Result<bool, AppError>;
}
