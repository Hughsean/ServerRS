use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_event_ingestion")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub source_event_id: String,
    pub connection_epoch_id: String,
    pub observed_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
