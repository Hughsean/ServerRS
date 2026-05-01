use sea_orm::entity::prelude::*;
use serde::Serialize;

/// SeaORM entity for the `users` table (MySQL).
///
/// This lives in the infrastructure layer and is NOT exposed to domain/application.
/// Mapping to `domain::user::user::User` happens in the repository implementation.
#[derive(Clone, Debug, DeriveEntityModel, Serialize)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: u64,
    pub username: String,
    pub password: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    #[sea_orm(column_type = "Blob")]
    pub avatar: Option<Vec<u8>>,
    pub nickname: Option<String>,
    pub status: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_login_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
