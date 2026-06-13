use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::domain::user::user::{NewUser, User, UserStatus, UserUpdate};
use crate::domain::user::user_repository::UserRepository;
use crate::shared::error::AppError;

use super::super::entities::users;

pub struct SeaOrmUserRepository {
    db: DatabaseConnection,
}

impl SeaOrmUserRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

// ── Mapping helpers ──

fn model_to_domain(m: users::Model) -> User {
    User {
        id: m.id,
        username: m.username,
        password_hash: m.password,
        email: m.email,
        phone: m.phone,
        nickname: m.nickname,
        status: UserStatus::from_i32(m.status as i32).unwrap_or(UserStatus::Disabled),
        role: crate::domain::user::user::UserRole::from_str(&m.role)
            .unwrap_or(crate::domain::user::user::UserRole::User),
        created_at: m.created_at,
        updated_at: m.updated_at,
        last_login_at: m.last_login_at,
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
impl UserRepository for SeaOrmUserRepository {
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

    async fn find_all(&self) -> Result<Vec<User>, AppError> {
        users::Entity::find()
            .all(&self.db)
            .await
            .map_err(map_db_err)
            .map(|rows| rows.into_iter().map(model_to_domain).collect())
    }

    async fn save(&self, new_user: NewUser) -> Result<User, AppError> {
        let now = chrono::Utc::now();
        let model = users::ActiveModel {
            username: Set(new_user.username),
            password: Set(new_user.password_hash),
            email: Set(new_user.email),
            phone: Set(new_user.phone),
            nickname: Set(new_user.nickname),
            status: Set(new_user.status.to_i32() as i8),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        };

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
        active.updated_at = Set(chrono::Utc::now());

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
        active.last_login_at = Set(Some(chrono::Utc::now()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
