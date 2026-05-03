use axum::{Extension, Json, extract::Path, extract::State};
use validator::Validate;

use crate::api::ApiState;
use crate::api::dto::user_dto::{
    DeleteUserResponse, UpdateUserRequest, UpsertUserProfileRequest, UserProfileResponse,
    UserResponse,
};
use crate::application::auth::auth_service::AuthenticatedUser;
use crate::domain::user::user::UserStatus;
use crate::shared::error::AppError;

// ── GET /api/v1/users/:user_id (profile) ──

pub async fn get_user_profile(
    State(state): State<ApiState>,
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

// ── PUT /api/v1/users/:user_id ──

pub async fn update_user(
    State(state): State<ApiState>,
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

// ── DELETE /api/v1/users/:user_id ──

pub async fn delete_user(
    State(state): State<ApiState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(user_id): Path<u64>,
) -> Result<Json<DeleteUserResponse>, AppError> {
    let deleted = state.user.delete_user(auth_user.user_id, user_id).await?;

    Ok(Json(DeleteUserResponse { deleted }))
}

// ── GET /api/v1/users ──

pub async fn list_users(
    State(state): State<ApiState>,
) -> Result<Json<Vec<UserResponse>>, AppError> {
    let users = state.user.list_users().await?;
    let response: Vec<UserResponse> = users.into_iter().map(user_to_response).collect();
    Ok(Json(response))
}

// ── PUT /api/v1/users/:user_id/profile ──

pub async fn upsert_user_profile(
    State(state): State<ApiState>,
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

// ── Helpers ──

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
