use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_source_events")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub source_event_id: String,
    pub account_id: u64,
    pub conversation_id: u64,
    pub source_channel: String,
    pub platform_event_id: String,
    pub event_type: String,
    pub actor_platform_id: String,
    pub actor_kind: String,
    pub message_role: String,
    pub occurred_at_unix_secs: i64,
    pub reply_to_platform_event_id: Option<String>,
    pub reply_to_event_id: Option<String>,
    pub processing_status: String,
    pub received_at: DateTime,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
