use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::api::UserState;
use crate::api::dto::user_dto::{
    DeleteUserResponse, UpdateUserRequest, UpsertUserProfileRequest, UserProfileResponse,
    UserResponse,
};
use crate::api::error::ApiError as AppError;
use crate::app::auth::auth_service::{AuthenticatedUser, Role};
use crate::domain::user::user::UserStatus;

// ── /me DTOs ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDto {
    pub id: u64,
    pub username: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub nickname: Option<String>,
    pub role: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct PatchMeRequest {
    #[validate(email)]
    pub email: Option<String>,
    #[validate(length(max = 20))]
    pub phone: Option<String>,
    #[validate(length(min = 1, max = 50))]
    pub nickname: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserProfileDto {
    pub user_id: u64,
    pub nickname: Option<String>,
    pub interests: Option<Vec<String>>,
    pub personality_traits: Option<Vec<String>>,
    pub interaction_preferences: Option<Vec<String>>,
    pub emotional_tendency: Option<Vec<String>>,
    pub learning_records: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct UpsertProfileRequest {
    #[validate(length(min = 1, max = 50))]
    pub nickname: Option<String>,
    pub interests: Option<Vec<String>>,
    pub personality_traits: Option<Vec<String>>,
    pub interaction_preferences: Option<Vec<String>>,
    pub emotional_tendency: Option<Vec<String>>,
    pub learning_records: Option<Vec<String>>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

pub async fn get_me(
    Extension(auth_user): Extension<AuthenticatedUser>,
    State(state): State<UserState>,
) -> Result<Json<UserDto>, AppError> {
    let user = state.user.get_user(auth_user.user_id).await?;
    Ok(Json(to_user_dto(user, &auth_user.role)))
}

pub async fn patch_me(
    Extension(auth_user): Extension<AuthenticatedUser>,
    State(state): State<UserState>,
    Json(payload): Json<PatchMeRequest>,
) -> Result<Json<UserDto>, AppError> {
    // 校验请求参数
    payload.validate().map_err(AppError::validation)?;
    let user = state
        .user
        .update_user(
            auth_user.user_id,
            auth_user.user_id,
            payload.email.map(Some),
            payload.phone.map(Some),
            payload.nickname.map(Some),
            None,
        )
        .await?;
    Ok(Json(to_user_dto(user, &auth_user.role)))
}

pub async fn delete_me(
    Extension(auth_user): Extension<AuthenticatedUser>,
    State(state): State<UserState>,
) -> Result<StatusCode, AppError> {
    state
        .user
        .delete_user(auth_user.user_id, auth_user.user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_profile(
    Extension(auth_user): Extension<AuthenticatedUser>,
    State(state): State<UserState>,
) -> Result<Json<UserProfileDto>, AppError> {
    let profile = state.user.get_profile(auth_user.user_id).await?;
    let nickname = state
        .user
        .admin_get_user(auth_user.user_id)
        .await?
        .and_then(|user| user.nickname);
    Ok(Json(to_profile_dto(profile, nickname)))
}

pub async fn put_profile(
    Extension(auth_user): Extension<AuthenticatedUser>,
    State(state): State<UserState>,
    Json(payload): Json<UpsertProfileRequest>,
) -> Result<Json<UserProfileDto>, AppError> {
    // 校验请求参数
    payload.validate().map_err(AppError::validation)?;
    let nickname = if let Some(nickname) = payload.nickname {
        state
            .user
            .update_user(
                auth_user.user_id,
                auth_user.user_id,
                None,
                None,
                Some(Some(nickname)),
                None,
            )
            .await?
            .nickname
    } else {
        state
            .user
            .admin_get_user(auth_user.user_id)
            .await?
            .and_then(|user| user.nickname)
    };
    let profile = state
        .user
        .upsert_profile(
            auth_user.user_id,
            payload.interests,
            payload.personality_traits,
            payload.interaction_preferences,
            payload.emotional_tendency,
            payload.learning_records,
        )
        .await?;
    Ok(Json(to_profile_dto(profile, nickname)))
}

// ── Legacy handlers (kept for existing router.rs routes) ─────────────────────

pub async fn get_user_profile(
    State(state): State<UserState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(user_id): Path<u64>,
) -> Result<Json<UserProfileResponse>, AppError> {
    if auth_user.user_id != user_id {
        return Err(AppError::Forbidden(format!(
            "user {} is not allowed to access user {}",
            auth_user.username, user_id
        )));
    }
    let result = state.user.get_profile(user_id).await?;
    Ok(Json(UserProfileResponse {
        id: result.id,
        user_id: result.user_id,
        interests: result.interests,
        personality_traits: result.personality_traits,
        interaction_preferences: result.interaction_preferences,
        emotional_tendency: result.emotional_tendency,
        learning_records: result.learning_records,
        created_at: result.created_at.to_rfc3339(),
        updated_at: result.updated_at.to_rfc3339(),
    }))
}

pub async fn update_user(
    State(state): State<UserState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(user_id): Path<u64>,
    Json(payload): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>, AppError> {
    payload.validate().map_err(AppError::validation)?;
    let status = payload
        .status
        .map(|s| UserStatus::from_i32(s).unwrap_or(UserStatus::Disabled));
    let user = state
        .user
        .update_user(
            auth_user.user_id,
            user_id,
            payload.email,
            payload.phone,
            payload.nickname,
            status,
        )
        .await?;
    Ok(Json(user_to_response(user)))
}

pub async fn delete_user(
    State(state): State<UserState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(user_id): Path<u64>,
) -> Result<Json<DeleteUserResponse>, AppError> {
    let deleted = state.user.delete_user(auth_user.user_id, user_id).await?;
    Ok(Json(DeleteUserResponse { deleted }))
}

pub async fn upsert_user_profile(
    State(state): State<UserState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(user_id): Path<u64>,
    Json(payload): Json<UpsertUserProfileRequest>,
) -> Result<Json<UserProfileResponse>, AppError> {
    if auth_user.user_id != user_id {
        return Err(AppError::Forbidden(format!(
            "user {} is not allowed to modify user {}'s profile",
            auth_user.username, user_id
        )));
    }
    let result = state
        .user
        .upsert_profile(
            user_id,
            payload.interests,
            payload.personality_traits,
            payload.interaction_preferences,
            payload.emotional_tendency,
            payload.learning_records,
        )
        .await?;
    Ok(Json(UserProfileResponse {
        id: result.id,
        user_id: result.user_id,
        interests: result.interests,
        personality_traits: result.personality_traits,
        interaction_preferences: result.interaction_preferences,
        emotional_tendency: result.emotional_tendency,
        learning_records: result.learning_records,
        created_at: result.created_at.to_rfc3339(),
        updated_at: result.updated_at.to_rfc3339(),
    }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn to_user_dto(u: crate::domain::user::user::User, role: &Role) -> UserDto {
    UserDto {
        id: u.id,
        username: u.username,
        email: u.email,
        phone: u.phone,
        nickname: u.nickname,
        role: role.to_string(),
        status: u.status.as_str().to_string(),
        created_at: u.created_at.to_rfc3339(),
    }
}

fn to_profile_dto(
    p: crate::domain::user::user_profile::UserProfile,
    nickname: Option<String>,
) -> UserProfileDto {
    UserProfileDto {
        user_id: p.user_id,
        nickname,
        interests: p.interests,
        personality_traits: p.personality_traits,
        interaction_preferences: p.interaction_preferences,
        emotional_tendency: p.emotional_tendency,
        learning_records: p.learning_records,
    }
}

fn user_to_response(u: crate::domain::user::user::User) -> UserResponse {
    UserResponse {
        id: u.id,
        username: u.username,
        email: u.email,
        phone: u.phone,
        nickname: u.nickname,
        status: u.status.as_str().to_string(),
        created_at: u.created_at.to_rfc3339(),
        updated_at: u.updated_at.to_rfc3339(),
        last_login_at: u.last_login_at.map(|t| t.to_rfc3339()),
    }
}
