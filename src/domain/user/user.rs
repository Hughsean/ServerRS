use chrono::{DateTime, Utc};
use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    Active,
    Disabled,
}

impl UserStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }

    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            1 => Some(Self::Active),
            0 => Some(Self::Disabled),
            _ => None,
        }
    }

    pub fn to_i32(self) -> i32 {
        match self {
            Self::Active => 1,
            Self::Disabled => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserRole {
    User,
    Admin,
    SuperAdmin,
}

impl UserRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "USER",
            Self::Admin => "ADMIN",
            Self::SuperAdmin => "SUPER_ADMIN",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "USER" => Some(Self::User),
            "ADMIN" => Some(Self::Admin),
            "SUPER_ADMIN" => Some(Self::SuperAdmin),
            _ => None,
        }
    }
}

impl fmt::Display for UserRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct NewUser {
    pub username: String,
    pub password_hash: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub nickname: Option<String>,
    pub status: UserStatus,
    pub role: UserRole,
}

impl NewUser {
    pub fn new(
        username: impl Into<String>,
        password_hash: impl Into<String>,
        status: UserStatus,
    ) -> Self {
        Self {
            username: username.into(),
            password_hash: password_hash.into(),
            email: None,
            phone: None,
            nickname: None,
            status,
            role: UserRole::User,
        }
    }

    pub fn with_email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    pub fn with_phone(mut self, phone: impl Into<String>) -> Self {
        self.phone = Some(phone.into());
        self
    }

    pub fn with_nickname(mut self, nickname: impl Into<String>) -> Self {
        self.nickname = Some(nickname.into());
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: u64,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub nickname: Option<String>,
    pub status: UserStatus,
    pub role: UserRole,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

impl User {
    pub fn is_active(&self) -> bool {
        matches!(self.status, UserStatus::Active)
    }

    pub fn is_disabled(&self) -> bool {
        matches!(self.status, UserStatus::Disabled)
    }
}

/// Partial update payload for a user (all fields optional).
#[derive(Debug, Clone)]
pub struct UserUpdate {
    pub email: Option<Option<String>>,
    pub phone: Option<Option<String>>,
    pub nickname: Option<Option<String>>,
    pub status: Option<UserStatus>,
    pub role: Option<UserRole>,
}

impl UserUpdate {
    pub fn has_any(&self) -> bool {
        self.email.is_some()
            || self.phone.is_some()
            || self.nickname.is_some()
            || self.status.is_some()
            || self.role.is_some()
    }
}
