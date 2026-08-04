use sea_orm::entity::prelude::*;

/// uncertain Gap 的再次领取退避时间；为 NULL 或已过期即立即可领取。
/// 本实体当前仅用于 schema 同步；读写通过原生 SQL 完成。
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secretary_gap_reclaim_schedule")]
#[allow(dead_code)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub gap_id: String,
    pub next_eligible_at: Option<DateTime>,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
#[allow(dead_code)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
