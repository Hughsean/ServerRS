use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, TryIntoModel};

use crate::domain::qq_bot::config::GroupMember;
use crate::domain::qq_bot::repository::GroupMemberRepository;
use crate::shared::error::AppError;

use super::super::super::persistence::entities::qq_group_members;

pub struct SeaOrmGroupMemberRepository {
    db: DatabaseConnection,
}

impl SeaOrmGroupMemberRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn model_to_domain(m: qq_group_members::Model) -> GroupMember {
    GroupMember {
        qq_group_id: m.qq_group_id,
        qq_user_id: m.qq_user_id,
        card: m.card,
        nickname: m.nickname,
        role: m.role,
        title: m.title,
        join_time: m.join_time,
        last_seen_at: m.last_seen_at,
        status: m.status,
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl GroupMemberRepository for SeaOrmGroupMemberRepository {
    async fn find(&self, qq_group_id: i64, qq_user_id: i64) -> Result<Option<GroupMember>, AppError> {
        qq_group_members::Entity::find_by_id((qq_group_id, qq_user_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn upsert(&self, member: &GroupMember) -> Result<GroupMember, AppError> {
        let model = qq_group_members::ActiveModel {
            qq_group_id: Set(member.qq_group_id),
            qq_user_id: Set(member.qq_user_id),
            card: Set(member.card.clone()),
            nickname: Set(member.nickname.clone()),
            role: Set(member.role.clone()),
            title: Set(member.title.clone()),
            join_time: Set(member.join_time),
            last_seen_at: Set(member.last_seen_at),
            status: Set(member.status.clone()),
            ..Default::default()
        };
        let result = model.save(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(result.try_into_model().unwrap()))
    }

    async fn update_last_seen(&self, qq_group_id: i64, qq_user_id: i64, _last_seen_at: i64) -> Result<(), AppError> {
        use sea_orm::sea_query::SimpleExpr;
        qq_group_members::Entity::update_many()
            .col_expr(
                qq_group_members::Column::LastSeenAt,
                SimpleExpr::Value(sea_orm::Value::BigInt(Some(_last_seen_at))),
            )
            .filter(qq_group_members::Column::QqGroupId.eq(qq_group_id))
            .filter(qq_group_members::Column::QqUserId.eq(qq_user_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn list_by_group(&self, qq_group_id: i64) -> Result<Vec<GroupMember>, AppError> {
        qq_group_members::Entity::find()
            .filter(qq_group_members::Column::QqGroupId.eq(qq_group_id))
            .all(&self.db)
            .await
            .map_err(map_db_err)
            .map(|rows| rows.into_iter().map(model_to_domain).collect())
    }
}
