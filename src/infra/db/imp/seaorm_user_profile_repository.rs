use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
    sea_query::OnConflict,
};

use crate::domain::user::user_profile::{NewUserProfile, UserProfile, UserProfileUpdate};
use crate::domain::user::user_profile_repository::UserProfileRepository;
use crate::shared::error::AppError;

use super::super::entities::user_profiles;

pub struct SeaOrmUserProfileRepository {
    db: DatabaseConnection,
}

impl SeaOrmUserProfileRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

// ── JSON helpers ──

fn parse_json_array(v: &Option<serde_json::Value>) -> Option<Vec<String>> {
    v.as_ref()
        .and_then(|j| serde_json::from_value::<Vec<String>>(j.clone()).ok())
}

fn to_json_array(v: &Option<Vec<String>>) -> Option<serde_json::Value> {
    v.as_ref()
        .map(|arr| serde_json::to_value(arr).unwrap_or(serde_json::Value::Null))
}

// ── Mapping ──

fn model_to_domain(m: user_profiles::Model) -> UserProfile {
    UserProfile {
        id: m.id,
        user_id: m.user_id,
        interests: parse_json_array(&m.interests),
        personality_traits: parse_json_array(&m.personality_traits),
        interaction_preferences: parse_json_array(&m.interaction_preferences),
        emotional_tendency: parse_json_array(&m.emotional_tendency),
        learning_records: parse_json_array(&m.learning_records),
        personalization_enabled: m.personalization_enabled != 0,
        personalization_reset_at: m.personalization_reset_at.map(|value| value.and_utc()),
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    let msg = e.to_string();
    if msg.contains("Duplicate entry") {
        AppError::Conflict("user profile already exists".into())
    } else {
        AppError::Internal(e.to_string())
    }
}

// ── Repository implementation ──

#[async_trait]
impl UserProfileRepository for SeaOrmUserProfileRepository {
    async fn find_by_user_id(&self, user_id: u64) -> Result<Option<UserProfile>, AppError> {
        user_profiles::Entity::find()
            .filter(user_profiles::Column::UserId.eq(user_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn save(&self, profile: NewUserProfile) -> Result<UserProfile, AppError> {
        let now = chrono::Utc::now();
        let model = user_profiles::ActiveModel {
            user_id: Set(profile.user_id),
            interests: Set(to_json_array(&profile.interests)),
            personality_traits: Set(to_json_array(&profile.personality_traits)),
            interaction_preferences: Set(to_json_array(&profile.interaction_preferences)),
            emotional_tendency: Set(to_json_array(&profile.emotional_tendency)),
            learning_records: Set(to_json_array(&profile.learning_records)),
            created_at: Set(now.naive_utc()),
            updated_at: Set(now.naive_utc()),
            ..Default::default()
        };

        let result = model.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(result))
    }

    async fn update(
        &self,
        user_id: u64,
        update: UserProfileUpdate,
    ) -> Result<UserProfile, AppError> {
        let existing = user_profiles::Entity::find()
            .filter(user_profiles::Column::UserId.eq(user_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or(AppError::NotFound("user profile not found".into()))?;

        let mut active: user_profiles::ActiveModel = existing.into();

        if let Some(interests) = update.interests {
            active.interests = Set(to_json_array(&interests));
        }
        if let Some(personality_traits) = update.personality_traits {
            active.personality_traits = Set(to_json_array(&personality_traits));
        }
        if let Some(interaction_preferences) = update.interaction_preferences {
            active.interaction_preferences = Set(to_json_array(&interaction_preferences));
        }
        if let Some(emotional_tendency) = update.emotional_tendency {
            active.emotional_tendency = Set(to_json_array(&emotional_tendency));
        }
        if let Some(learning_records) = update.learning_records {
            active.learning_records = Set(to_json_array(&learning_records));
        }
        active.updated_at = Set(chrono::Utc::now().naive_utc());

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(updated))
    }

    /// 原子化创建或更新用户画像。
    /// 使用 INSERT ... ON DUPLICATE KEY UPDATE 消除 TOCTOU 竞态。
    async fn upsert(&self, user_id: u64, profile: NewUserProfile) -> Result<UserProfile, AppError> {
        let now = chrono::Utc::now().naive_utc();
        let model = user_profiles::ActiveModel {
            user_id: Set(profile.user_id),
            interests: Set(to_json_array(&profile.interests)),
            personality_traits: Set(to_json_array(&profile.personality_traits)),
            interaction_preferences: Set(to_json_array(&profile.interaction_preferences)),
            emotional_tendency: Set(to_json_array(&profile.emotional_tendency)),
            learning_records: Set(to_json_array(&profile.learning_records)),
            personalization_enabled: Set(0),
            personalization_reset_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        };

        // 使用 SeaORM 的 insert_many + on_conflict 实现原子化 upsert，
        // 确保在并发场景下不会产生 Duplicate Entry 错误。
        user_profiles::Entity::insert_many([model])
            .on_conflict(
                OnConflict::columns([user_profiles::Column::UserId])
                    .update_columns([
                        user_profiles::Column::Interests,
                        user_profiles::Column::PersonalityTraits,
                        user_profiles::Column::InteractionPreferences,
                        user_profiles::Column::EmotionalTendency,
                        user_profiles::Column::LearningRecords,
                        user_profiles::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        // 执行后重新查询以返回完整领域对象
        let result = user_profiles::Entity::find()
            .filter(user_profiles::Column::UserId.eq(user_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| AppError::Internal("upsert 后未找到用户画像".into()))?;

        Ok(model_to_domain(result))
    }

    async fn delete_by_user_id(&self, user_id: u64) -> Result<bool, AppError> {
        let result = user_profiles::Entity::delete_many()
            .filter(user_profiles::Column::UserId.eq(user_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected > 0)
    }
}
