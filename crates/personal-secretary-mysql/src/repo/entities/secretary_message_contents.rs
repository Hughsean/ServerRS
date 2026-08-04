use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_message_contents")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub source_event_id: String,
    pub normalized_text: String,
    pub segments: Json,
    pub mentioned_actor_ids: Json,
    pub mention_all: bool,
    pub content_mode: String,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
