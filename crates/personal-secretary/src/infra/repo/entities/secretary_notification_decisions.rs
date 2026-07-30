use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_notification_decisions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub notification_decision_id: String,
    pub evaluation_request_id: String,
    pub notification_candidate_id: String,
    pub previous_decision_id: Option<String>,
    pub policy_revision_id: Option<String>,
    pub evaluator_version: String,
    pub outcome: String,
    pub reason_code: String,
    pub next_allowed_at_unix_secs: Option<i64>,
    pub created_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
