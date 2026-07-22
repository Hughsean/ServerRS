//! HTTP Chat Agent 的可恢复运行快照。

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "agent_checkpoints")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub checkpoint_id: String,
    pub run_id: String,
    pub graph_id: String,
    pub graph_version: u32,
    pub state_schema_version: u32,
    pub user_id: u64,
    pub conversation_id: u64,
    pub next_node: String,
    pub completed_step: u32,
    pub suspend_reason: String,
    pub payload: Json,
    pub status: String,
    pub expires_at: DateTime,
    pub consumed_at: Option<DateTime>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    #[sea_orm(
        belongs_to,
        from = "conversation_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    pub conversations: BelongsTo<super::conversations::Entity>,
    #[sea_orm(
        belongs_to,
        from = "user_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    pub users: BelongsTo<super::users::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
