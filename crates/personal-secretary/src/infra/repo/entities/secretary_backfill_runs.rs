use sea_orm::entity::prelude::*;

/// 一次历史回补运行的持久化状态：Gap、租约、进度、完整性证据与终态。
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_backfill_runs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub backfill_run_id: String,
    pub gap_id: String,
    pub account_id: u64,
    pub connection_epoch_id: String,
    pub status: String,
    pub lease_expires_at: Option<DateTime>,
    pub completeness: String,
    pub failure_class: Option<String>,
    pub pages_read: u32,
    pub events_read: u32,
    pub accepted: u32,
    pub duplicates: u32,
    pub budget_exhausted: bool,
    pub anomaly_count: u32,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub completed_at: Option<DateTime>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::secretary_backfill_scopes::Entity")]
    BackfillScopes,
}

impl Related<super::secretary_backfill_scopes::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::BackfillScopes.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
