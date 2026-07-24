use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_thread_link_reviews")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub review_id: String,
    pub candidate_id: String,
    pub review_action: String,
    pub owner_channel: String,
    pub owner_account: String,
    pub owner_actor_id: String,
    pub command_source_event_id: String,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
