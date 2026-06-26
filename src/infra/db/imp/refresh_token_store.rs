use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::domain::auth::refresh_token_store::RefreshTokenStoreT;
use crate::shared::error::AppError;

use super::super::entities::refresh_tokens;

pub struct RefreshTokenStoreImpl {
    db: DatabaseConnection,
    refresh_ttl_secs: u64,
}

impl RefreshTokenStoreImpl {
    pub fn new(db: DatabaseConnection, refresh_ttl_secs: u64) -> Self {
        Self {
            db,
            refresh_ttl_secs,
        }
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    let msg = e.to_string();
    if msg.contains("Duplicate entry") || msg.contains("UNIQUE") {
        if msg.contains("token_id") {
            AppError::Conflict("refresh token id already exists".into())
        } else if msg.contains("token_hash") {
            AppError::Conflict("refresh token hash already exists".into())
        } else {
            AppError::Conflict(msg)
        }
    } else {
        AppError::Internal(e.to_string())
    }
}

#[async_trait]
impl RefreshTokenStoreT for RefreshTokenStoreImpl {
    async fn store(&self, user_id: u64, token_hash: String) -> Result<(), AppError> {
        let token_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().naive_utc();
        let expires_at = now
            .and_utc()
            .timestamp()
            .saturating_add_unsigned(self.refresh_ttl_secs) as u64;

        refresh_tokens::ActiveModel {
            refresh_token_id: Set(0), // auto-increment
            token_id: Set(token_id),
            user_id: Set(user_id),
            token_hash: Set(token_hash),
            expires_at: Set(expires_at),
            revoked_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&self.db)
        .await
        .map_err(map_db_err)?;

        Ok(())
    }

    async fn is_revoked(&self, token_hash: &str) -> Result<bool, AppError> {
        let now_ts = Utc::now().timestamp() as u64;

        let found = refresh_tokens::Entity::find()
            .filter(refresh_tokens::Column::TokenHash.eq(token_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        match found {
            None => Ok(true), // unknown => treat as revoked
            Some(record) => {
                if record.revoked_at.is_some() {
                    return Ok(true);
                }
                if record.expires_at < now_ts {
                    return Ok(true);
                }
                Ok(false)
            }
        }
    }

    async fn revoke(&self, token_hash: &str) -> Result<(), AppError> {
        let now = Utc::now().naive_utc();

        let record = refresh_tokens::Entity::find()
            .filter(refresh_tokens::Column::TokenHash.eq(token_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        if let Some(record) = record {
            let mut active: refresh_tokens::ActiveModel = record.into();
            active.revoked_at = Set(Some(now));
            active.updated_at = Set(now);
            active.update(&self.db).await.map_err(map_db_err)?;
        }

        Ok(())
    }

    async fn cleanup_expired(&self, now_seconds: u64) -> Result<usize, AppError> {
        let result = refresh_tokens::Entity::delete_many()
            .filter(refresh_tokens::Column::ExpiresAt.lt(now_seconds))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(result.rows_affected as usize)
    }
}
