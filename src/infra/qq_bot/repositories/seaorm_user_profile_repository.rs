use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::domain::qq_bot::qq_profile_repository::QqUserProfileRepository;
use crate::domain::qq_bot::user_profile::UserProfile;
use crate::shared::error::AppError;

use crate::infra::db::entities::qq_user_profiles;

pub struct SeaOrmQqUserProfileRepository {
    db: DatabaseConnection,
}

impl SeaOrmQqUserProfileRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn model_to_domain(m: qq_user_profiles::Model) -> UserProfile {
    UserProfile {
        qq_user_id: m.qq_user_id,
        interest_tags: m
            .interest_tags
            .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok()),
        active_hours: m.active_hours,
        speaking_style: m.speaking_style,
        topic_frequency: m.topic_frequency,
        total_messages: m.total_messages,
        avg_message_length: m.avg_message_length,
        emoji_usage_rate: m.emoji_usage_rate,
        first_seen_at: m.first_seen_at,
        last_summary_at: m.last_summary_at,
        raw_profile: m.raw_profile,
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl QqUserProfileRepository for SeaOrmQqUserProfileRepository {
    async fn find_by_qq_user_id(&self, qq_user_id: i64) -> Result<Option<UserProfile>, AppError> {
        qq_user_profiles::Entity::find_by_id(qq_user_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn upsert(&self, profile: &UserProfile) -> Result<UserProfile, AppError> {
        let interest_tags = profile
            .interest_tags
            .as_ref()
            .map(|tags| serde_json::to_value(tags).unwrap_or(serde_json::Value::Null));

        let model = qq_user_profiles::ActiveModel {
            qq_user_id: Set(profile.qq_user_id),
            interest_tags: Set(interest_tags),
            active_hours: Set(profile.active_hours.clone()),
            speaking_style: Set(profile.speaking_style.clone()),
            topic_frequency: Set(profile.topic_frequency.clone()),
            total_messages: Set(profile.total_messages),
            avg_message_length: Set(profile.avg_message_length),
            emoji_usage_rate: Set(profile.emoji_usage_rate),
            first_seen_at: Set(profile.first_seen_at),
            last_summary_at: Set(profile.last_summary_at),
            raw_profile: Set(profile.raw_profile.clone()),
            ..Default::default()
        };

        let update_columns = vec![
            qq_user_profiles::Column::InterestTags,
            qq_user_profiles::Column::ActiveHours,
            qq_user_profiles::Column::SpeakingStyle,
            qq_user_profiles::Column::TopicFrequency,
            qq_user_profiles::Column::TotalMessages,
            qq_user_profiles::Column::AvgMessageLength,
            qq_user_profiles::Column::EmojiUsageRate,
            qq_user_profiles::Column::FirstSeenAt,
            qq_user_profiles::Column::LastSummaryAt,
            qq_user_profiles::Column::RawProfile,
        ];

        qq_user_profiles::Entity::insert_many([model])
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([qq_user_profiles::Column::QqUserId])
                    .update_columns(update_columns)
                    .to_owned(),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        self.find_by_qq_user_id(profile.qq_user_id)
            .await?
            .ok_or_else(|| AppError::Internal("user profile not found after upsert".into()))
    }

    async fn update_stats(
        &self,
        qq_user_id: i64,
        total_messages: u32,
        avg_message_length: f64,
        emoji_usage_rate: f64,
    ) -> Result<(), AppError> {
        use sea_orm::sea_query::SimpleExpr;
        qq_user_profiles::Entity::update_many()
            .col_expr(
                qq_user_profiles::Column::TotalMessages,
                SimpleExpr::Value(sea_orm::Value::Unsigned(Some(total_messages))),
            )
            .col_expr(
                qq_user_profiles::Column::AvgMessageLength,
                SimpleExpr::Value(sea_orm::Value::Double(Some(avg_message_length))),
            )
            .col_expr(
                qq_user_profiles::Column::EmojiUsageRate,
                SimpleExpr::Value(sea_orm::Value::Double(Some(emoji_usage_rate))),
            )
            .filter(qq_user_profiles::Column::QqUserId.eq(qq_user_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn update_summary_at(
        &self,
        qq_user_id: i64,
        last_summary_at: i64,
    ) -> Result<(), AppError> {
        use sea_orm::sea_query::SimpleExpr;
        qq_user_profiles::Entity::update_many()
            .col_expr(
                qq_user_profiles::Column::LastSummaryAt,
                SimpleExpr::Value(sea_orm::Value::BigInt(Some(last_summary_at))),
            )
            .filter(qq_user_profiles::Column::QqUserId.eq(qq_user_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }
}
