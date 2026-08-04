use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_ingestion_cursors")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: u64,
    pub account_id: u64,
    pub conversation_id: Option<u64>,
    pub scope_kind: String,
    pub scope_key: String,
    pub last_source_event_id: String,
    pub last_platform_event_id: String,
    pub last_occurred_at_unix_secs: i64,
    pub connection_epoch_id: Option<String>,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
