use serde::{Deserialize, Serialize};
use validator::Validate;

// ── Profile ──

#[derive(Debug, Serialize)]
pub struct UserProfileResponse {
    pub id: u64,
    pub user_id: u64,
    pub interests: Option<Vec<String>>,
    pub personality_traits: Option<Vec<String>>,
    pub interaction_preferences: Option<Vec<String>>,
    pub emotional_tendency: Option<Vec<String>>,
    pub learning_records: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct UpsertUserProfileRequest {
    pub interests: Option<Vec<String>>,
    pub personality_traits: Option<Vec<String>>,
    pub interaction_preferences: Option<Vec<String>>,
    pub emotional_tendency: Option<Vec<String>>,
    pub learning_records: Option<Vec<String>>,
}

// ── User CRUD ──

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: u64,
    pub username: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub nickname: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_login_at: Option<String>,
}

#[derive(Debug, Deserialize, Validate)]
pub struct UpdateUserRequest {
    pub email: Option<Option<String>>,
    pub phone: Option<Option<String>>,
    pub nickname: Option<Option<String>>,
    #[validate(range(min = 0, max = 1))]
    pub status: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct DeleteUserResponse {
    pub deleted: bool,
}
