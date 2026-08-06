use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_connection_epochs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub connection_epoch_id: String,
    pub account_id: u64,
    pub source_channel: String,
    pub status: String,
    pub started_at: DateTime,
    pub connected_at: Option<DateTime>,
    pub ended_at: Option<DateTime>,
    pub last_event_at: Option<DateTime>,
    pub last_source_event_id: Option<String>,
    pub end_reason: Option<String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
