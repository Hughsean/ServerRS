use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_accounts")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: u64,
    pub source_channel: String,
    pub platform_account_id: String,
    pub status: String,
    pub policy_epoch: u64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
