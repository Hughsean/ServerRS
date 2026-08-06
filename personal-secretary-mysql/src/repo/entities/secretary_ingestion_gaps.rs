use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_ingestion_gaps")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub gap_id: String,
    pub account_id: u64,
    pub connection_epoch_id: String,
    pub gap_started_at: DateTime,
    pub gap_ended_at: Option<DateTime>,
    pub status: String,
    pub reason: String,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
