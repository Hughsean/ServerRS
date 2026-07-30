use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_notification_candidates")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub notification_candidate_id: String,
    pub account_id: u64,
    pub source_kind: String,
    pub source_id: String,
    pub source_version: u64,
    pub match_key_json: Json,
    pub candidate_status: String,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
