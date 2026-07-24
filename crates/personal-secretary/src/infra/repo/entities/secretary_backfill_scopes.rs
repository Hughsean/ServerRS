use sea_orm::entity::prelude::*;

/// 单个会话 Scope 的回补进度与证据；锚点与边界均绑定账号视角平台消息 ID。
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_backfill_scopes")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: u64,
    pub backfill_run_id: String,
    pub account_id: u64,
    pub conversation_id: u64,
    pub scope_kind: String,
    pub scope_key: String,
    pub status: String,
    pub last_anchor_message_id: Option<String>,
    pub last_anchor_message_seq: Option<String>,
    pub pages_read: u32,
    pub events_read: u32,
    pub accepted: u32,
    pub duplicates: u32,
    pub reached_boundary: bool,
    pub anomalies: Option<Json>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::secretary_backfill_runs::Entity",
        from = "Column::BackfillRunId",
        to = "super::secretary_backfill_runs::Column::BackfillRunId"
    )]
    BackfillRun,
}

impl Related<super::secretary_backfill_runs::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::BackfillRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
