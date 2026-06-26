use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Set,
    Statement,
};

use crate::domain::user::user_context_version::{
    ContextVersionReason, UserContextVersion, UserContextVersionRepoT,
};
use crate::shared::error::AppError;

use super::super::entities::user_context_versions;

pub struct UserContextVersionRepo {
    db: DatabaseConnection,
}

impl UserContextVersionRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn map_model(model: user_context_versions::Model) -> UserContextVersion {
    UserContextVersion {
        user_id: model.user_id,
        version: model.version,
        updated_at: model.updated_at.and_utc(),
    }
}

#[async_trait]
impl UserContextVersionRepoT for UserContextVersionRepo {
    async fn get_or_create(&self, user_id: u64) -> Result<UserContextVersion, AppError> {
        if let Some(model) = user_context_versions::Entity::find_by_id(user_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find context version: {e}")))?
        {
            return Ok(map_model(model));
        }

        let now = Utc::now().naive_utc();
        let active = user_context_versions::ActiveModel {
            user_id: Set(user_id),
            version: Set(1),
            updated_at: Set(now),
        };
        match active.insert(&self.db).await {
            Ok(model) => Ok(map_model(model)),
            Err(_) => user_context_versions::Entity::find_by_id(user_id)
                .one(&self.db)
                .await
                .map_err(|e| AppError::internal(format!("reload context version: {e}")))?
                .map(map_model)
                .ok_or_else(|| AppError::internal("context version insert was not visible")),
        }
    }

    async fn bump(&self, user_id: u64, _reason: ContextVersionReason) -> Result<u64, AppError> {
        let statement = Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO user_context_versions (user_id, version, updated_at) \
             VALUES (?, 2, UTC_TIMESTAMP(6)) \
             ON DUPLICATE KEY UPDATE version = version + 1, updated_at = UTC_TIMESTAMP(6)",
            [user_id.into()],
        );
        self.db
            .execute_raw(statement)
            .await
            .map_err(|e| AppError::internal(format!("bump context version: {e}")))?;

        user_context_versions::Entity::find_by_id(user_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("read bumped context version: {e}")))?
            .map(|model| model.version)
            .ok_or_else(|| AppError::internal("bumped context version was not visible"))
    }
}
