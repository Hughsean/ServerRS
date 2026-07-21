use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

use crate::api::CommunityState;
use crate::api::error::ApiError as AppError;
use crate::app::auth::auth_service::AuthenticatedUser;

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct PostDto {
    pub post_id: u64,
    pub user_id: u64,
    pub title: Option<String>,
    pub content: String,
    pub likes_count: u32,
    pub comments_count: u32,
}

#[derive(Serialize)]
pub struct CommentDto {
    pub comment_id: u64,
    pub post_id: u64,
    pub user_id: u64,
    pub parent_comment_id: Option<u64>,
    pub content: String,
    pub likes_count: u32,
}

#[derive(Serialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
}

#[derive(Deserialize)]
pub struct PageQuery {
    pub page: Option<u64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<u64>,
}

#[derive(Deserialize)]
pub struct CreatePostRequest {
    pub title: Option<String>,
    pub content: String,
}

#[derive(Deserialize)]
pub struct UpdatePostRequest {
    pub title: Option<String>,
    pub content: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateCommentRequest {
    pub content: String,
    pub parent_comment_id: Option<u64>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn post_to_dto(p: &crate::domain::community::Post) -> PostDto {
    PostDto {
        post_id: p.post_id,
        user_id: p.user_id,
        title: p.title.clone(),
        content: p.content.clone(),
        likes_count: p.likes_count,
        comments_count: p.comments_count,
    }
}

fn comment_to_dto(c: &crate::domain::community::Comment) -> CommentDto {
    CommentDto {
        comment_id: c.comment_id,
        post_id: c.post_id,
        user_id: c.user_id,
        parent_comment_id: c.parent_comment_id,
        content: c.content.clone(),
        likes_count: c.likes_count,
    }
}

// ── Posts ─────────────────────────────────────────────────────────────────────

pub async fn list_posts(
    State(state): State<CommunityState>,
    Query(q): Query<PageQuery>,
) -> Result<Json<PaginatedResponse<PostDto>>, AppError> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).max(1).min(100);
    let (items, total) = state.community.list_posts(page, page_size).await?;
    let dtos: Vec<PostDto> = items.iter().map(post_to_dto).collect();
    Ok(Json(PaginatedResponse {
        items: dtos,
        page,
        page_size,
        total,
    }))
}

pub async fn get_post(
    State(state): State<CommunityState>,
    Path(id): Path<u64>,
) -> Result<Json<PostDto>, AppError> {
    let post = state.community.get_post(id).await?;
    Ok(Json(post_to_dto(&post)))
}

pub async fn create_post(
    State(state): State<CommunityState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<CreatePostRequest>,
) -> Result<impl IntoResponse, AppError> {
    let post = state
        .community
        .create_post(auth.user_id, body.title, body.content)
        .await?;
    Ok((StatusCode::CREATED, Json(post_to_dto(&post))))
}

pub async fn update_post(
    State(state): State<CommunityState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(id): Path<u64>,
    Json(body): Json<UpdatePostRequest>,
) -> Result<Json<PostDto>, AppError> {
    let post = state
        .community
        .update_post(id, auth.user_id, body.title, body.content)
        .await?;
    Ok(Json(post_to_dto(&post)))
}

pub async fn delete_post(
    State(state): State<CommunityState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(id): Path<u64>,
) -> Result<StatusCode, AppError> {
    state.community.delete_post(id, auth.user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Comments ──────────────────────────────────────────────────────────────────

pub async fn list_comments(
    State(state): State<CommunityState>,
    Path(post_id): Path<u64>,
    Query(q): Query<PageQuery>,
) -> Result<Json<PaginatedResponse<CommentDto>>, AppError> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).max(1).min(100);
    let (items, total) = state
        .community
        .list_comments(post_id, page, page_size)
        .await?;
    let dtos: Vec<CommentDto> = items.iter().map(comment_to_dto).collect();
    Ok(Json(PaginatedResponse {
        items: dtos,
        page,
        page_size,
        total,
    }))
}

pub async fn create_comment(
    State(state): State<CommunityState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(post_id): Path<u64>,
    Json(body): Json<CreateCommentRequest>,
) -> Result<impl IntoResponse, AppError> {
    let comment = state
        .community
        .create_comment(post_id, auth.user_id, body.content, body.parent_comment_id)
        .await?;
    Ok((StatusCode::CREATED, Json(comment_to_dto(&comment))))
}

pub async fn delete_comment(
    State(state): State<CommunityState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path((post_id, comment_id)): Path<(u64, u64)>,
) -> Result<StatusCode, AppError> {
    state
        .community
        .delete_comment(post_id, comment_id, auth.user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Like / Unlike ─────────────────────────────────────────────────────────────

pub async fn like_post(
    State(state): State<CommunityState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(post_id): Path<u64>,
) -> Result<StatusCode, AppError> {
    state.community.like_post(post_id, auth.user_id).await?;
    Ok(StatusCode::OK)
}

pub async fn unlike_post(
    State(state): State<CommunityState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(post_id): Path<u64>,
) -> Result<StatusCode, AppError> {
    state.community.unlike_post(post_id, auth.user_id).await?;
    Ok(StatusCode::OK)
}

pub async fn like_comment(
    State(state): State<CommunityState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path((post_id, comment_id)): Path<(u64, u64)>,
) -> Result<StatusCode, AppError> {
    state
        .community
        .like_comment(post_id, comment_id, auth.user_id)
        .await?;
    Ok(StatusCode::OK)
}

pub async fn unlike_comment(
    State(state): State<CommunityState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path((post_id, comment_id)): Path<(u64, u64)>,
) -> Result<StatusCode, AppError> {
    state
        .community
        .unlike_comment(post_id, comment_id, auth.user_id)
        .await?;
    Ok(StatusCode::OK)
}
