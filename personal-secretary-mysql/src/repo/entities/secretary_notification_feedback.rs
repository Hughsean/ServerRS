use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_notification_feedback")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub feedback_id: String,
    pub account_id: u64,
    pub notification_candidate_id: String,
    pub important: bool,
    pub promote_to_rule: bool,
    pub command_source_event_id: String,
    pub audit_summary: String,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
