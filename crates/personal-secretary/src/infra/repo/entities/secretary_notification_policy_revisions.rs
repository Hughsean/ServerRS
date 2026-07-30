use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_notification_policy_revisions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub policy_revision_id: String,
    pub policy_family_id: String,
    pub revision_number: u64,
    pub supersedes_revision_id: Option<String>,
    pub revision_kind: String,
    pub rule_json: Option<Json>,
    pub command_source_event_id: Option<String>,
    pub audit_summary: String,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
