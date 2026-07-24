use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_thread_link_candidates")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub candidate_id: String,
    pub account_id: u64,
    pub left_thread_id: String,
    pub right_thread_id: String,
    pub left_conversation_id: u64,
    pub right_conversation_id: u64,
    pub signal_kind: String,
    pub fingerprint_sha256: String,
    pub status: String,
    pub confidence_bps: u16,
    pub reason_code: String,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
