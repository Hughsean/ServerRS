use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DerivePartialModel,
    EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set, Statement, Value,
};

use crate::domain::user::user::{
    NewUser, QQ_AUTO_REGISTERED_SENTINEL, User, UserListItem, UserStatus, UserUpdate,
};
use crate::domain::user::user_repo::UserRepoT;
use crate::shared::error::AppError;

use super::super::entities::users;

pub struct UserRepo {
    db: DatabaseConnection,
}

impl UserRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

// ── Mapping helpers ──

fn model_to_domain(m: users::Model) -> User {
    User {
        id: m.id,
        username: m.username,
        password_hash: if m.password == QQ_AUTO_REGISTERED_SENTINEL {
            None
        } else {
            Some(m.password)
        },
        email: m.email,
        phone: m.phone,
        nickname: m.nickname,
        status: UserStatus::from_i32(m.status as i32).unwrap_or(UserStatus::Disabled),
        role: crate::domain::user::user::UserRole::from_str(&m.role)
            .unwrap_or(crate::domain::user::user::UserRole::User),
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
        last_login_at: m.last_login_at.map(|v| v.and_utc()),
    }
}

#[derive(DerivePartialModel)]
#[sea_orm(entity = "users::Entity")]
struct UserListRow {
    id: u64,
    username: String,
    email: Option<String>,
    phone: Option<String>,
    nickname: Option<String>,
    status: i8,
    role: String,
    created_at: chrono::NaiveDateTime,
    updated_at: chrono::NaiveDateTime,
    last_login_at: Option<chrono::NaiveDateTime>,
}

fn list_row_to_domain(row: UserListRow) -> UserListItem {
    UserListItem {
        id: row.id,
        username: row.username,
        email: row.email,
        phone: row.phone,
        nickname: row.nickname,
        status: UserStatus::from_i32(row.status as i32).unwrap_or(UserStatus::Disabled),
        role: crate::domain::user::user::UserRole::from_str(&row.role)
            .unwrap_or(crate::domain::user::user::UserRole::User),
        created_at: row.created_at.and_utc(),
        updated_at: row.updated_at.and_utc(),
        last_login_at: row.last_login_at.map(|value| value.and_utc()),
    }
}

fn map_db_err(e: sea_orm::DbErr) -> AppError {
    let msg = e.to_string();
    if msg.contains("Duplicate entry") || msg.contains("UNIQUE") {
        if msg.contains("username") {
            AppError::Conflict("username already exists".into())
        } else if msg.contains("email") {
            AppError::Conflict("email already in use".into())
        } else if msg.contains("phone") {
            AppError::Conflict("phone already in use".into())
        } else {
            AppError::Conflict(msg)
        }
    } else {
        AppError::Internal(e.to_string())
    }
}

// ── Repository implementation ──

#[async_trait]
impl UserRepoT for UserRepo {
    async fn find_by_id(&self, id: u64) -> Result<Option<User>, AppError> {
        users::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn find_by_username(&self, username: &str) -> Result<Option<User>, AppError> {
        users::Entity::find()
            .filter(users::Column::Username.eq(username))
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<User>, AppError> {
        users::Entity::find()
            .filter(users::Column::Email.eq(email))
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn find_by_phone(&self, phone: &str) -> Result<Option<User>, AppError> {
        users::Entity::find()
            .filter(users::Column::Phone.eq(phone))
            .one(&self.db)
            .await
            .map_err(map_db_err)
            .map(|opt| opt.map(model_to_domain))
    }

    async fn find_all_paginated(
        &self,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<UserListItem>, u64), AppError> {
        let paginator = users::Entity::find()
            .order_by_desc(users::Column::CreatedAt)
            .into_partial_model::<UserListRow>()
            .paginate(&self.db, limit);
        let total = paginator.num_items().await.map_err(map_db_err)?;
        let rows = paginator
            .fetch_page(offset / limit)
            .await
            .map_err(map_db_err)?;
        Ok((rows.into_iter().map(list_row_to_domain).collect(), total))
    }

    async fn save(&self, new_user: NewUser) -> Result<User, AppError> {
        let now = chrono::Utc::now();
        let model: users::ActiveModel = users::ActiveModel::builder()
            .set_username(new_user.username)
            .set_password(
                new_user
                    .password_hash
                    .unwrap_or_else(|| QQ_AUTO_REGISTERED_SENTINEL.to_string()),
            )
            .set_email(new_user.email)
            .set_phone(new_user.phone)
            .set_nickname(new_user.nickname)
            .set_status(new_user.status.to_i32() as i8)
            .set_role(new_user.role.as_str())
            .set_created_at(now.naive_utc())
            .set_updated_at(now.naive_utc())
            .into();

        let result = model.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(result))
    }

    async fn update(&self, id: u64, update: UserUpdate) -> Result<User, AppError> {
        // Fetch existing
        let existing = users::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or(AppError::NotFound("users not found".into()))?;

        let mut active: users::ActiveModel = existing.into();

        if let Some(email) = update.email {
            active.email = Set(email);
        }
        if let Some(phone) = update.phone {
            active.phone = Set(phone);
        }
        if let Some(nickname) = update.nickname {
            active.nickname = Set(nickname);
        }
        if let Some(status) = update.status {
            active.status = Set(status.to_i32() as i8);
        }
        if let Some(role) = update.role {
            active.role = Set(role.as_str().to_string());
        }
        active.updated_at = Set(chrono::Utc::now().naive_utc());

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_domain(updated))
    }

    async fn delete_by_id(&self, id: u64) -> Result<bool, AppError> {
        let result = users::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected > 0)
    }

    async fn update_last_login(&self, id: u64) -> Result<(), AppError> {
        let existing = users::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or(AppError::NotFound("users not found".into()))?;

        let mut active: users::ActiveModel = existing.into();
        active.last_login_at = Set(Some(chrono::Utc::now().naive_utc()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn count_all(&self) -> Result<u64, AppError> {
        users::Entity::find()
            .count(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn count_trend(&self, days: u32) -> Result<Vec<(String, u64)>, AppError> {
        let since = chrono::Utc::now() - chrono::Duration::days(days as i64 - 1);
        let start = since.format("%Y-%m-%d").to_string();
        let stmt = Statement::from_sql_and_values(
            self.db.get_database_backend(),
            r#"
            SELECT DATE(created_at) AS day, COUNT(*) AS cnt
            FROM users
            WHERE created_at >= CAST(? AS DATETIME)
            GROUP BY DATE(created_at)
            ORDER BY day
            "#,
            vec![Value::String(Some(start))],
        );
        let rows = self.db.query_all_raw(stmt).await.map_err(map_db_err)?;
        let mut daily: Vec<(String, u64)> = rows
            .into_iter()
            .filter_map(|row| {
                let day: String = row.try_get("", "day").ok()?;
                let cnt: i64 = row.try_get("", "cnt").ok()?;
                Some((day, cnt as u64))
            })
            .collect();
        Ok(fill_trend_daily(days, &mut daily))
    }
}

/// Fill missing days with 0 for trend data.
fn fill_trend_daily(days: u32, daily: &mut [(String, u64)]) -> Vec<(String, u64)> {
    daily.sort_by(|a, b| a.0.cmp(&b.0));
    let mut result = Vec::with_capacity(days as usize);
    let today = chrono::Utc::now().date_naive();
    for i in (0..days).rev() {
        let date = today - chrono::Duration::days(i as i64);
        let label = date.format("%m-%d").to_string();
        let full = date.format("%Y-%m-%d").to_string();
        let count = daily
            .iter()
            .find(|(d, _)| *d == full)
            .map(|(_, c)| *c)
            .unwrap_or(0);
        result.push((label, count));
    }
    result
}
