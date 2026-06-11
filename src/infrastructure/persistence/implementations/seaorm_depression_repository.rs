use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};

use crate::domain::depression::{
    DepressionAssessment, DepressionRepository, DepressionScale, NewDepressionAssessment,
};
use crate::shared::error::AppError;

use super::super::entities::{depression_assessments, depression_scales};

fn map_scale(m: depression_scales::Model) -> DepressionScale {
    DepressionScale {
        scale_id: m.scale_id,
        scale_name: m.scale_name,
        scale_description: m.scale_description,
        min_score: m.min_score,
        max_score: m.max_score,
        questions: m.questions,
        severity_ranges: m.severity_ranges,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

fn map_assessment(m: depression_assessments::Model) -> DepressionAssessment {
    DepressionAssessment {
        assessment_id: m.assessment_id,
        user_id: m.user_id,
        scale_id: m.scale_id,
        assessment_date: m.assessment_date,
        answers: m.answers,
        total_score: m.total_score,
        notes: m.notes,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

pub struct SeaOrmDepressionRepository {
    db: DatabaseConnection,
}

impl SeaOrmDepressionRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl DepressionRepository for SeaOrmDepressionRepository {
    async fn find_scale_by_id(&self, id: u16) -> Result<Option<DepressionScale>, AppError> {
        depression_scales::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|o| o.map(map_scale))
    }

    async fn list_scales(&self) -> Result<Vec<DepressionScale>, AppError> {
        depression_scales::Entity::find()
            .order_by_asc(depression_scales::Column::ScaleId)
            .all(&self.db)
            .await
            .map_err(map_err)
            .map(|v| v.into_iter().map(map_scale).collect())
    }

    async fn save_assessment(
        &self,
        new: NewDepressionAssessment,
        total_score: i16,
    ) -> Result<DepressionAssessment, AppError> {
        let now = chrono::Utc::now();
        let am = depression_assessments::ActiveModel {
            user_id: Set(new.user_id),
            scale_id: Set(new.scale_id),
            assessment_date: Set(chrono::Utc::now().date_naive()),
            answers: Set(new.answers),
            total_score: Set(total_score),
            notes: Set(new.notes),
            created_at: Set(Some(now)),
            updated_at: Set(Some(now)),
            ..Default::default()
        };
        Ok(map_assessment(am.insert(&self.db).await.map_err(map_err)?))
    }

    async fn find_assessment_by_id(
        &self,
        id: u64,
    ) -> Result<Option<DepressionAssessment>, AppError> {
        depression_assessments::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|o| o.map(map_assessment))
    }

    async fn find_assessments_by_user_id(
        &self,
        user_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<DepressionAssessment>, u64), AppError> {
        let paginator = depression_assessments::Entity::find()
            .filter(depression_assessments::Column::UserId.eq(user_id))
            .order_by_desc(depression_assessments::Column::AssessmentDate)
            .paginate(&self.db, limit);
        let count = paginator.num_items().await.map_err(map_err)?;
        let page_num = offset / limit;
        let items = paginator.fetch_page(page_num).await.map_err(map_err)?;
        Ok((items.into_iter().map(map_assessment).collect(), count))
    }

    async fn update_assessment(
        &self,
        id: u64,
        notes: Option<String>,
    ) -> Result<DepressionAssessment, AppError> {
        let existing = depression_assessments::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or(AppError::NotFound("assessment not found".into()))?;
        let mut am: depression_assessments::ActiveModel = existing.into();
        am.notes = Set(notes);
        am.updated_at = Set(Some(chrono::Utc::now()));
        Ok(map_assessment(am.update(&self.db).await.map_err(map_err)?))
    }

    async fn delete_assessment(&self, id: u64) -> Result<u64, AppError> {
        Ok(depression_assessments::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .map_err(map_err)?
            .rows_affected)
    }
}
