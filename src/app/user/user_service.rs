use std::sync::Arc;

use crate::domain::user::user::{User, UserStatus, UserUpdate};
use crate::domain::user::user_profile::{NewUserProfile, UserProfile};
use crate::domain::user::user_profile_repository::UserProfileRepository;
use crate::domain::user::user_repository::UserRepository;
use crate::shared::error::AppError;

/// Unified user + profile operations.  Replaces 5 separate use-case files.
pub struct UserService {
    user_repo: Arc<dyn UserRepository>,
    profile_repo: Arc<dyn UserProfileRepository>,
}

impl UserService {
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        profile_repo: Arc<dyn UserProfileRepository>,
    ) -> Self {
        Self {
            user_repo,
            profile_repo,
        }
    }

    // ── User CRUD ──

    /// 根据 ID 查询用户（当前用户自身）。
    pub async fn get_user(&self, user_id: u64) -> Result<User, AppError> {
        self.user_repo
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::NotFound("user not found".into()))
    }

    pub async fn list_users(&self) -> Result<Vec<User>, AppError> {
        self.user_repo.find_all().await
    }

    pub async fn update_user(
        &self,
        actor_user_id: u64,
        user_id: u64,
        email: Option<Option<String>>,
        phone: Option<Option<String>>,
        nickname: Option<Option<String>>,
        status: Option<UserStatus>,
    ) -> Result<User, AppError> {
        if actor_user_id != user_id {
            return Err(AppError::Forbidden(
                "you can only update your own profile".into(),
            ));
        }

        let update = UserUpdate {
            email,
            phone,
            nickname,
            status,
            role: None,
        };

        if !update.has_any() {
            return self
                .user_repo
                .find_by_id(user_id)
                .await?
                .ok_or(AppError::NotFound("user not found".into()));
        }

        self.user_repo.update(user_id, update).await
    }

    pub async fn admin_get_user(&self, user_id: u64) -> Result<Option<User>, AppError> {
        self.user_repo.find_by_id(user_id).await
    }

    pub async fn admin_update_user(
        &self,
        user_id: u64,
        update: UserUpdate,
    ) -> Result<User, AppError> {
        self.user_repo.update(user_id, update).await
    }

    pub async fn admin_delete_user(&self, user_id: u64) -> Result<(), AppError> {
        if self.user_repo.delete_by_id(user_id).await? {
            Ok(())
        } else {
            Err(AppError::NotFound(format!("user {user_id} not found")))
        }
    }

    pub async fn delete_user(&self, actor_user_id: u64, user_id: u64) -> Result<bool, AppError> {
        if actor_user_id != user_id {
            return Err(AppError::Forbidden(
                "you can only delete your own account".into(),
            ));
        }
        self.user_repo.delete_by_id(user_id).await
    }

    // ── Profile ──

    pub async fn get_profile(&self, user_id: u64) -> Result<UserProfile, AppError> {
        self.profile_repo
            .find_by_user_id(user_id)
            .await?
            .ok_or(AppError::NotFound("user profile not found".into()))
    }

    pub async fn upsert_profile(
        &self,
        user_id: u64,
        interests: Option<Vec<String>>,
        personality_traits: Option<Vec<String>>,
        interaction_preferences: Option<Vec<String>>,
        emotional_tendency: Option<Vec<String>>,
        learning_records: Option<Vec<String>>,
    ) -> Result<UserProfile, AppError> {
        let new_profile = NewUserProfile {
            user_id,
            interests,
            personality_traits,
            interaction_preferences,
            emotional_tendency,
            learning_records,
        };
        // 使用原子化 upsert 替代 find-then-save/update，消除 TOCTOU 竞态
        self.profile_repo.upsert(user_id, new_profile).await
    }
}
