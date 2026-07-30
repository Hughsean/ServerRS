use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_notification_evaluation_requests")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub evaluation_request_id: String,
    pub notification_candidate_id: String,
    pub evaluation_generation: u64,
    pub trigger_kind: String,
    pub request_status: String,
    pub lease_token: Option<String>,
    pub lease_expires_at_unix_secs: Option<i64>,
    pub attempt: u64,
    pub next_allowed_at_unix_secs: Option<i64>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
