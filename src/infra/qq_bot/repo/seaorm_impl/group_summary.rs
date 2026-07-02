use async_trait::async_trait;
use sea_orm::sea_query::SimpleExpr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, Value,
};

use crate::domain::qq_bot::repository::{GroupSummary, GroupSummaryRepoT};
use crate::shared::error::AppError;

use crate::infra::repo::entities::qq_group_summaries;

pub struct GroupSummaryRepo {
    db: DatabaseConnection,
}

impl GroupSummaryRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn model_to_domain(m: qq_group_summaries::Model) -> GroupSummary {
    GroupSummary {
        summary_id: Some(m.summary_id),
        qq_group_id: m.qq_group_id,
        summary_type: m.summary_type,
        content: m.content,
        message_start_id: m.message_start_id,
        message_end_id: m.message_end_id,
        supersedes_id: m.supersedes_id,
        token_count: m.token_count,
        status: m.status != 0,
        vector_id: m.vector_id,
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl GroupSummaryRepoT for GroupSummaryRepo {
    async fn find_active_rolling(
        &self,
        qq_group_id: i64,
    ) -> Result<Option<GroupSummary>, AppError> {
        qq_group_summaries::Entity::find()
            .filter(qq_group_summaries::Column::QqGroupId.eq(qq_group_id))
            .filter(qq_group_summaries::Column::SummaryType.eq("rolling_group"))
            .filter(qq_group_summaries::Column::Status.eq(1))
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn insert(&self, summary: &GroupSummary) -> Result<GroupSummary, AppError> {
        let model = qq_group_summaries::ActiveModel {
            qq_group_id: Set(summary.qq_group_id),
            summary_type: Set(summary.summary_type.clone()),
            content: Set(summary.content.clone()),
            message_start_id: Set(summary.message_start_id),
            message_end_id: Set(summary.message_end_id),
            supersedes_id: Set(summary.supersedes_id),
            token_count: Set(summary.token_count),
            status: Set(if summary.status { 1i8 } else { 0i8 }),
            vector_id: Set(summary.vector_id.clone()),
            ..Default::default()
        };
        let result = model.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(result))
    }

    async fn disable(&self, summary_id: u64) -> Result<(), AppError> {
        qq_group_summaries::Entity::update_many()
            .col_expr(
                qq_group_summaries::Column::Status,
                SimpleExpr::Value(Value::TinyInt(Some(0i8))),
            )
            .filter(qq_group_summaries::Column::SummaryId.eq(summary_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }
}
