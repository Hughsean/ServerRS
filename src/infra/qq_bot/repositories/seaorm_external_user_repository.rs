use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, TryIntoModel,
};

use crate::domain::qq_bot::config::ExternalUser;
use crate::domain::qq_bot::repository::ExternalUserRepository;
use crate::shared::error::AppError;

use super::super::super::persistence::entities::qq_external_users;

pub struct SeaOrmExternalUserRepository {
    db: DatabaseConnection,
}

impl SeaOrmExternalUserRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn model_to_domain(m: qq_external_users::Model) -> ExternalUser {
    ExternalUser {
        qq_user_id: m.qq_user_id,
        internal_user_id: m.internal_user_id,
        nickname: m.nickname,
        avatar_url: m.avatar_url,
        last_seen_at: m.last_seen_at,
        memory_enabled: m.memory_enabled != 0,
        persona_enabled: m.persona_enabled != 0,
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl ExternalUserRepository for SeaOrmExternalUserRepository {
    async fn find_by_qq_user_id(&self, qq_user_id: i64) -> Result<Option<ExternalUser>, AppError> {
        qq_external_users::Entity::find_by_id(qq_user_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn upsert(&self, user: &ExternalUser) -> Result<ExternalUser, AppError> {
        let model = qq_external_users::ActiveModel {
            qq_user_id: Set(user.qq_user_id),
            internal_user_id: Set(user.internal_user_id),
            nickname: Set(user.nickname.clone()),
            avatar_url: Set(user.avatar_url.clone()),
            last_seen_at: Set(user.last_seen_at),
            memory_enabled: Set(if user.memory_enabled { 1i8 } else { 0i8 }),
            persona_enabled: Set(if user.persona_enabled { 1i8 } else { 0i8 }),
            ..Default::default()
        };
        let result = model.save(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(result.try_into_model().unwrap()))
    }

    async fn update_last_seen(&self, qq_user_id: i64, last_seen_at: i64) -> Result<(), AppError> {
        use sea_orm::sea_query::SimpleExpr;
        qq_external_users::Entity::update_many()
            .col_expr(
                qq_external_users::Column::LastSeenAt,
                SimpleExpr::Value(sea_orm::Value::BigInt(Some(last_seen_at))),
            )
            .filter(qq_external_users::Column::QqUserId.eq(qq_user_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }
}
