use sea_orm::entity::prelude::*;
use serde::Serialize;

/// SeaORM entity for the `user_profiles` table (MySQL).
#[derive(Clone, Debug, DeriveEntityModel, Serialize)]
#[sea_orm(table_name = "user_profiles")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: u64,
    pub user_id: u64,
    /// JSON array stored as text
    pub interests: Option<String>,
    /// JSON array stored as text
    pub personality_traits: Option<String>,
    /// JSON array stored as text
    pub interaction_preferences: Option<String>,
    /// JSON array stored as text
    pub emotional_tendency: Option<String>,
    /// JSON array stored as text
    pub learning_records: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
