use std::sync::Arc;

use crate::domain::user::user::{User, UserStatus, UserUpdate};
use crate::domain::user::user_profile::{NewUserProfile, UserProfile, UserProfileUpdate};
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
        let existing = self.profile_repo.find_by_user_id(user_id).await?;

        match existing {
            Some(_) => {
                let update = UserProfileUpdate {
                    interests: Some(interests),
                    personality_traits: Some(personality_traits),
                    interaction_preferences: Some(interaction_preferences),
                    emotional_tendency: Some(emotional_tendency),
                    learning_records: Some(learning_records),
                };
                self.profile_repo.update(user_id, update).await
            }
            None => {
                let new_profile = NewUserProfile {
                    user_id,
                    interests,
                    personality_traits,
                    interaction_preferences,
                    emotional_tendency,
                    learning_records,
                };
                self.profile_repo.save(new_profile).await
            }
        }
    }
}
