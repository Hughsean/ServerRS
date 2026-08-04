use sea_orm::entity::prelude::*;

/// Gap 创建时冻结的会话游标快照；回补边界按平台消息 ID 匹配，非领取时实时游标。
/// 本实体当前仅用于 schema 同步；读写通过原生 SQL 完成。
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_gap_boundaries")]
#[allow(dead_code)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: u64,
    pub gap_id: String,
    pub account_id: u64,
    pub conversation_id: u64,
    pub conversation_kind: String,
    pub platform_conversation_id: String,
    pub boundary_message_id: String,
    pub boundary_occurred_at_unix_secs: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
#[allow(dead_code)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
