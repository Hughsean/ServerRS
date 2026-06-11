use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};

use crate::api::ApiState;
use crate::application::auth::auth_service::AuthenticatedUser;
use crate::shared::error::AppError;

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryDto {
    pub category_id: u64,
    pub category_name: String,
    pub parent_id: Option<u64>,
    pub children: Vec<CategoryDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArticleDto {
    pub article_id: u64,
    pub title: String,
    pub summary: Option<String>,
    pub author: Option<String>,
    pub tags: Option<String>,
    pub view_count: i64,
    pub like_count: i64,
    pub is_featured: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QnaDto {
    pub qna_id: u64,
    pub question: String,
    pub answer: String,
    pub expert_name: Option<String>,
    pub is_verified: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDto {
    pub resource_id: u64,
    pub resource_type: String,
    pub title: String,
    pub file_size: Option<i64>,
    pub mime_type: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedResponse<T: Serialize> {
    pub items: Vec<T>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageQuery {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    pub category_id: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToggleFavoriteRequest {
    pub content_type: String,
    pub content_id: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteDto {
    pub id: u64,
    pub content_type: String,
    pub content_id: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteStatusDto {
    pub favorited: bool,
}

// ── Categories ────────────────────────────────────────────────────────────────

pub async fn list_categories(
    State(state): State<ApiState>,
) -> Result<Json<Vec<CategoryDto>>, AppError> {
    let categories = state.psychology.list_categories().await?;
    let dtos = categories
        .into_iter()
        .map(|c| CategoryDto {
            category_id: c.id,
            category_name: c.name,
            parent_id: c.parent_id,
            children: Vec::new(),
        })
        .collect();
    Ok(Json(dtos))
}

pub async fn get_category_tree(
    State(state): State<ApiState>,
) -> Result<Json<Vec<CategoryDto>>, AppError> {
    let categories = state.psychology.list_categories().await?;
    let tree = build_category_tree(categories, None);
    Ok(Json(tree))
}

fn build_category_tree(
    all: Vec<crate::domain::psychology::PsychologyCategory>,
    parent_id: Option<u64>,
) -> Vec<CategoryDto> {
    all.iter()
        .filter(|c| c.parent_id == parent_id)
        .map(|c| CategoryDto {
            category_id: c.id,
            category_name: c.name.clone(),
            parent_id: c.parent_id,
            children: build_category_tree(all.clone(), Some(c.id)),
        })
        .collect()
}

// ── Articles ──────────────────────────────────────────────────────────────────

pub async fn list_articles(
    State(state): State<ApiState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<PaginatedResponse<ArticleDto>>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).max(1).min(100);
    let (articles, total) = state
        .psychology
        .list_articles(page, page_size, None, query.category_id, None)
        .await?;
    let items = articles
        .into_iter()
        .map(|a| ArticleDto {
            article_id: a.id,
            title: a.title,
            summary: a.summary,
            author: None,
            tags: a.tags,
            view_count: a.view_count,
            like_count: a.like_count,
            is_featured: a.is_published,
        })
        .collect();
    Ok(Json(PaginatedResponse {
        items,
        page,
        page_size,
        total,
    }))
}

pub async fn get_article(
    State(state): State<ApiState>,
    Path(id): Path<u64>,
) -> Result<Json<ArticleDto>, AppError> {
    let article = state.psychology.get_article(id).await?;
    Ok(Json(ArticleDto {
        article_id: article.id,
        title: article.title,
        summary: article.summary,
        author: None,
        tags: article.tags,
        view_count: article.view_count,
        like_count: article.like_count,
        is_featured: article.is_published,
    }))
}

// ── QnA ───────────────────────────────────────────────────────────────────────

pub async fn list_qna(
    State(state): State<ApiState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<PaginatedResponse<QnaDto>>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).max(1).min(100);
    let (qnas, total) = state
        .psychology
        .list_qnas(page, page_size, query.category_id, None)
        .await?;
    let items = qnas
        .into_iter()
        .map(|q| QnaDto {
            qna_id: q.id,
            question: q.question,
            answer: q.answer,
            expert_name: None,
            is_verified: q.is_published,
        })
        .collect();
    Ok(Json(PaginatedResponse {
        items,
        page,
        page_size,
        total,
    }))
}

pub async fn get_qna(
    State(state): State<ApiState>,
    Path(id): Path<u64>,
) -> Result<Json<QnaDto>, AppError> {
    let qna = state.psychology.get_qna(id).await?;
    Ok(Json(QnaDto {
        qna_id: qna.id,
        question: qna.question,
        answer: qna.answer,
        expert_name: None,
        is_verified: qna.is_published,
    }))
}

// ── Resources ─────────────────────────────────────────────────────────────────

pub async fn list_resources(
    State(state): State<ApiState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<PaginatedResponse<ResourceDto>>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).max(1).min(100);
    let (resources, total) = state
        .psychology
        .list_resources(page, page_size, query.category_id, None)
        .await?;
    let items = resources
        .into_iter()
        .map(|r| ResourceDto {
            resource_id: r.id,
            resource_type: r.resource_type,
            title: r.title,
            file_size: None,
            mime_type: None,
        })
        .collect();
    Ok(Json(PaginatedResponse {
        items,
        page,
        page_size,
        total,
    }))
}

pub async fn get_resource(
    State(state): State<ApiState>,
    Path(id): Path<u64>,
) -> Result<Json<ResourceDto>, AppError> {
    let resource = state.psychology.get_resource(id).await?;
    Ok(Json(ResourceDto {
        resource_id: resource.id,
        resource_type: resource.resource_type,
        title: resource.title,
        file_size: None,
        mime_type: None,
    }))
}

// ── Favorites ─────────────────────────────────────────────────────────────────

pub async fn toggle_favorite(
    State(state): State<ApiState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Json(payload): Json<ToggleFavoriteRequest>,
) -> Result<Json<FavoriteStatusDto>, AppError> {
    let favorited = state
        .psychology
        .toggle_favorite(
            Some(auth_user.user_id),
            payload.content_type,
            payload.content_id,
        )
        .await?;
    Ok(Json(FavoriteStatusDto { favorited }))
}

pub async fn check_favorite(
    State(state): State<ApiState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Query(query): Query<CheckFavoriteQuery>,
) -> Result<Json<FavoriteStatusDto>, AppError> {
    let favorited = state
        .psychology
        .check_favorite(auth_user.user_id, &query.content_type, query.content_id)
        .await?;
    Ok(Json(FavoriteStatusDto { favorited }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckFavoriteQuery {
    pub content_type: String,
    pub content_id: u64,
}

pub async fn list_favorites(
    State(state): State<ApiState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Query(query): Query<PageQuery>,
) -> Result<Json<PaginatedResponse<FavoriteDto>>, AppError> {
    let content_type = query.category_id.map(|_| "article").or(None);
    let favorites = state
        .psychology
        .list_favorites(auth_user.user_id, content_type)
        .await?;
    let items: Vec<FavoriteDto> = favorites
        .into_iter()
        .map(|f| FavoriteDto {
            id: f.id,
            content_type: f.content_type,
            content_id: f.content_id,
        })
        .collect();
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).max(1).min(100);
    Ok(Json(PaginatedResponse {
        total: items.len() as u64,
        page,
        page_size,
        items,
    }))
}
