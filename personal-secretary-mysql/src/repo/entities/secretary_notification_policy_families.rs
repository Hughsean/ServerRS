use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_notification_policy_families")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub policy_family_id: String,
    pub account_id: u64,
    pub canonical_scope_key: String,
    pub policy_kind: String,
    pub current_revision_id: Option<String>,
    pub generation: u64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
