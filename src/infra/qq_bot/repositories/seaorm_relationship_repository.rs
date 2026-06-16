use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
};

use crate::domain::qq_bot::relationship::{RapportLevel, RelationshipState};
use crate::domain::qq_bot::relationship_repository::RelationshipRepository;
use crate::shared::error::AppError;

use super::super::super::persistence::entities::qq_relationships;

pub struct SeaOrmRelationshipRepository {
    db: DatabaseConnection,
}

impl SeaOrmRelationshipRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn model_to_domain(m: qq_relationships::Model) -> RelationshipState {
    RelationshipState {
        id: Some(m.id),
        qq_group_id: m.qq_group_id,
        qq_user_id: m.qq_user_id,
        familiarity: m.familiarity,
        interaction_count: m.interaction_count,
        last_interaction_at: m.last_interaction_at,
        rapport: serde_json::from_value(serde_json::json!(m.rapport))
            .unwrap_or(RapportLevel::Neutral),
        nickname_preference: m.nickname_preference,
        known_interests: m.known_interests
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default(),
        known_avoid_topics: m.known_avoid_topics
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default(),
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl RelationshipRepository for SeaOrmRelationshipRepository {
    async fn find(&self, qq_group_id: i64, qq_user_id: i64) -> Result<Option<RelationshipState>, AppError> {
        qq_relationships::Entity::find()
            .filter(qq_relationships::Column::QqGroupId.eq(qq_group_id))
            .filter(qq_relationships::Column::QqUserId.eq(qq_user_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn upsert(&self, rel: &RelationshipState) -> Result<RelationshipState, AppError> {
        // rapport is stored as string (e.g. "neutral", "friendly")
        let rapport_str = serde_json::to_value(&rel.rapport)
            .and_then(|v| Ok(v.as_str().unwrap_or("neutral").to_string()))
            .unwrap_or_else(|_| "neutral".to_string());

        let known_interests_json: Option<serde_json::Value> = if rel.known_interests.is_empty() {
            None
        } else {
            serde_json::to_value(&rel.known_interests).ok()
        };

        let known_avoid_topics_json: Option<serde_json::Value> = if rel.known_avoid_topics.is_empty() {
            None
        } else {
            serde_json::to_value(&rel.known_avoid_topics).ok()
        };

        // Try to find existing record first
        let existing = qq_relationships::Entity::find()
            .filter(qq_relationships::Column::QqGroupId.eq(rel.qq_group_id))
            .filter(qq_relationships::Column::QqUserId.eq(rel.qq_user_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        let result = if let Some(model) = existing {
            let mut active: qq_relationships::ActiveModel = model.into();
            active.familiarity = Set(rel.familiarity);
            active.interaction_count = Set(rel.interaction_count);
            active.last_interaction_at = Set(rel.last_interaction_at);
            active.rapport = Set(rapport_str);
            active.nickname_preference = Set(rel.nickname_preference.clone());
            active.known_interests = Set(known_interests_json);
            active.known_avoid_topics = Set(known_avoid_topics_json);
            active.update(&self.db).await.map_err(map_db_err)?
        } else {
            let active = qq_relationships::ActiveModel {
                id: sea_orm::ActiveValue::NotSet,
                qq_group_id: Set(rel.qq_group_id),
                qq_user_id: Set(rel.qq_user_id),
                familiarity: Set(rel.familiarity),
                interaction_count: Set(rel.interaction_count),
                last_interaction_at: Set(rel.last_interaction_at),
                rapport: Set(rapport_str),
                nickname_preference: Set(rel.nickname_preference.clone()),
                known_interests: Set(known_interests_json),
                known_avoid_topics: Set(known_avoid_topics_json),
                ..Default::default()
            };
            active.insert(&self.db).await.map_err(map_db_err)?
        };

        Ok(model_to_domain(result))
    }

    async fn find_by_group(&self, qq_group_id: i64) -> Result<Vec<RelationshipState>, AppError> {
        qq_relationships::Entity::find()
            .filter(qq_relationships::Column::QqGroupId.eq(qq_group_id))
            .all(&self.db)
            .await
            .map_err(map_db_err)
            .map(|models| models.into_iter().map(model_to_domain).collect())
    }

    async fn increment_interaction(&self, qq_group_id: i64, qq_user_id: i64) -> Result<(), AppError> {
        let existing = qq_relationships::Entity::find()
            .filter(qq_relationships::Column::QqGroupId.eq(qq_group_id))
            .filter(qq_relationships::Column::QqUserId.eq(qq_user_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        if let Some(model) = existing {
            let mut active: qq_relationships::ActiveModel = model.into();
            let current_count = active.interaction_count.as_ref().clone();
            let new_count = current_count + 1;
            active.interaction_count = Set(new_count);
            let familiarity = (0.1_f32 + new_count as f32 * 0.015).min(1.0);
            active.familiarity = Set(familiarity);
            active.last_interaction_at = Set(Some(chrono::Utc::now().timestamp()));
            active.rapport = Set(RapportLevel::from_familiarity(familiarity).label().to_string());
            active.update(&self.db).await.map_err(map_db_err)?;
        } else {
            let now = chrono::Utc::now().timestamp();
            let active = qq_relationships::ActiveModel {
                id: sea_orm::ActiveValue::NotSet,
                qq_group_id: Set(qq_group_id),
                qq_user_id: Set(qq_user_id),
                familiarity: Set(0.1),
                interaction_count: Set(1),
                last_interaction_at: Set(Some(now)),
                rapport: Set("neutral".to_string()),
                nickname_preference: Set(None),
                known_interests: Set(None),
                known_avoid_topics: Set(None),
                ..Default::default()
            };
            active.insert(&self.db).await.map_err(map_db_err)?;
        }

        Ok(())
    }
}
