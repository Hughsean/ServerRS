use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};

use crate::domain::diary::{DiaryRepoT, NewUserDiary, UserDiary, UserDiaryUpdate};
use crate::shared::error::AppError;

use super::super::entities::user_diaries;

fn map(m: user_diaries::Model) -> UserDiary {
    UserDiary {
        id: m.id,
        user_id: m.user_id,
        title: m.title,
        content: m.content,
        mood_description: m.mood_description,
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

pub struct DiaryRepo {
    db: DatabaseConnection,
}

impl DiaryRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl DiaryRepoT for DiaryRepo {
    async fn save(&self, diary: NewUserDiary) -> Result<UserDiary, AppError> {
        let now = chrono::Utc::now();
        let am: user_diaries::ActiveModel = user_diaries::ActiveModel::builder()
            .set_user_id(diary.user_id)
            .set_title(diary.title)
            .set_content(diary.content)
            .set_mood_description(None)
            .set_created_at(now.naive_utc())
            .set_updated_at(now.naive_utc())
            .into();
        Ok(map(am.insert(&self.db).await.map_err(map_err)?))
    }

    async fn find_by_id(&self, id: u64) -> Result<Option<UserDiary>, AppError> {
        user_diaries::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|o| o.map(map))
    }

    async fn find_by_user_id(
        &self,
        user_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<UserDiary>, u64), AppError> {
        let paginator = user_diaries::Entity::find()
            .filter(user_diaries::Column::UserId.eq(user_id))
            .order_by_desc(user_diaries::Column::CreatedAt)
            .paginate(&self.db, limit);
        let count = paginator.num_items().await.map_err(map_err)?;
        let page_num = offset / limit;
        let items = paginator.fetch_page(page_num).await.map_err(map_err)?;
        Ok((items.into_iter().map(map).collect(), count))
    }

    async fn update(&self, id: u64, update: UserDiaryUpdate) -> Result<UserDiary, AppError> {
        let existing = user_diaries::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or(AppError::NotFound("diary not found".into()))?;
        let mut am: user_diaries::ActiveModel = existing.into();
        if let Some(title) = update.title {
            am.title = Set(title);
        }
        if let Some(content) = update.content {
            am.content = Set(content);
        }
        if let Some(md) = update.mood_description {
            am.mood_description = Set(md);
        }
        am.updated_at = Set(chrono::Utc::now().naive_utc());
        Ok(map(am.update(&self.db).await.map_err(map_err)?))
    }

    async fn update_mood(&self, id: u64, mood_description: String) -> Result<(), AppError> {
        let existing = user_diaries::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or(AppError::NotFound("diary not found".into()))?;
        let mut am: user_diaries::ActiveModel = existing.into();
        am.mood_description = Set(Some(mood_description));
        am.updated_at = Set(chrono::Utc::now().naive_utc());
        am.update(&self.db).await.map_err(map_err)?;
        Ok(())
    }

    async fn delete_by_id(&self, id: u64) -> Result<bool, AppError> {
        Ok(user_diaries::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .map_err(map_err)?
            .rows_affected
            > 0)
    }
}
