use axum::{
    Json,
    extract::{Extension, Path, Query, State},
};
use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::api::DiaryState;
use crate::api::error::ApiError as AppError;
use crate::app::auth::auth_service::AuthenticatedUser;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiaryListQuery {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[derive(Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateDiaryRequest {
    #[validate(length(min = 1, max = 10000))]
    pub content: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDiaryRequest {
    pub title: Option<String>,
    pub content: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiaryDto {
    pub id: u64,
    pub user_id: u64,
    pub title: String,
    pub content: String,
    pub mood_description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<crate::domain::diary::UserDiary> for DiaryDto {
    fn from(d: crate::domain::diary::UserDiary) -> Self {
        Self {
            id: d.id,
            user_id: d.user_id,
            title: d.title,
            content: d.content,
            mood_description: d.mood_description,
            created_at: d.created_at.to_rfc3339(),
            updated_at: d.updated_at.to_rfc3339(),
        }
    }
}

pub async fn list_diaries(
    State(state): State<DiaryState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Query(q): Query<DiaryListQuery>,
) -> Result<Json<Vec<DiaryDto>>, AppError> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);

    let (diaries, _total) = state.diaries.list(auth.user_id, page, page_size).await?;

    Ok(Json(diaries.into_iter().map(DiaryDto::from).collect()))
}

pub async fn get_diary(
    State(state): State<DiaryState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(id): Path<u64>,
) -> Result<Json<DiaryDto>, AppError> {
    let diary = state.diaries.get(auth.user_id, id).await?;
    Ok(Json(diary.into()))
}

pub async fn create_diary(
    State(state): State<DiaryState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(payload): Json<CreateDiaryRequest>,
) -> Result<Json<DiaryDto>, AppError> {
    // 校验请求参数
    payload.validate().map_err(AppError::validation)?;
    let title = payload.title.unwrap_or_else(|| "无标题".to_string());
    let diary = state
        .diaries
        .create(auth.user_id, title, payload.content)
        .await?;
    Ok(Json(diary.into()))
}

pub async fn update_diary(
    State(state): State<DiaryState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(id): Path<u64>,
    Json(payload): Json<UpdateDiaryRequest>,
) -> Result<Json<DiaryDto>, AppError> {
    let diary = state
        .diaries
        .update(auth.user_id, id, payload.title, payload.content)
        .await?;
    Ok(Json(diary.into()))
}

pub async fn delete_diary(
    State(state): State<DiaryState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(id): Path<u64>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.diaries.delete(auth.user_id, id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}
