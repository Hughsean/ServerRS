use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};

use crate::domain::like::{ContentLike, ContentLikeRepository};
use crate::shared::error::AppError;

use super::super::entities::content_likes;

fn map(m: content_likes::Model) -> ContentLike {
    ContentLike {
        like_id: m.like_id,
        user_id: m.user_id,
        content_type: m.content_type,
        content_id: m.content_id,
        created_at: chrono::DateTime::from_naive_utc_and_offset(m.created_at, chrono::Utc),
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

pub struct SeaOrmLikeRepository {
    db: DatabaseConnection,
}

impl SeaOrmLikeRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ContentLikeRepository for SeaOrmLikeRepository {
    async fn toggle(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError> {
        let existing = content_likes::Entity::find()
            .filter(content_likes::Column::UserId.eq(user_id))
            .filter(content_likes::Column::ContentType.eq(content_type))
            .filter(content_likes::Column::ContentId.eq(content_id))
            .one(&self.db)
            .await
            .map_err(map_err)?;

        if let Some(record) = existing {
            content_likes::Entity::delete_by_id(record.like_id)
                .exec(&self.db)
                .await
                .map_err(map_err)?;
            Ok(false)
        } else {
            let am = content_likes::ActiveModel {
                user_id: Set(user_id),
                content_type: Set(content_type.to_owned()),
                content_id: Set(content_id),
                ..Default::default()
            };
            match am.insert(&self.db).await {
                Ok(_) => Ok(true),
                Err(sea_orm::DbErr::Exec(_)) => Ok(false), // unique constraint: already liked
                Err(e) => Err(map_err(e)),
            }
        }
    }

    async fn is_liked(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError> {
        let count = content_likes::Entity::find()
            .filter(content_likes::Column::UserId.eq(user_id))
            .filter(content_likes::Column::ContentType.eq(content_type))
            .filter(content_likes::Column::ContentId.eq(content_id))
            .count(&self.db)
            .await
            .map_err(map_err)?;
        Ok(count > 0)
    }

    async fn count_by_content(&self, content_type: &str, content_id: u64) -> Result<u64, AppError> {
        content_likes::Entity::find()
            .filter(content_likes::Column::ContentType.eq(content_type))
            .filter(content_likes::Column::ContentId.eq(content_id))
            .count(&self.db)
            .await
            .map_err(map_err)
    }

    async fn delete(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError> {
        let result = content_likes::Entity::delete_many()
            .filter(content_likes::Column::UserId.eq(user_id))
            .filter(content_likes::Column::ContentType.eq(content_type))
            .filter(content_likes::Column::ContentId.eq(content_id))
            .exec(&self.db)
            .await
            .map_err(map_err)?;
        Ok(result.rows_affected > 0)
    }
}
