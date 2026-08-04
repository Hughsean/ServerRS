use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_conversations")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: u64,
    pub account_id: u64,
    pub conversation_kind: String,
    pub platform_conversation_id: String,
    pub memory_mode: String,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
