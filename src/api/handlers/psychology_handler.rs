use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};

use crate::api::PsychologyState;
use crate::application::auth::auth_service::AuthenticatedUser;
use crate::domain::psychology::{
    NewPsychologyArticle, NewPsychologyCategory, NewPsychologyQna, NewPsychologyResource,
};
use crate::shared::error::AppError;

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryDto {
    pub category_id: u64,
    pub category_name: String,
    pub parent_id: Option<u64>,
    pub description: Option<String>,
    pub sort_order: i32,
    pub is_enabled: bool,
    pub children: Vec<CategoryDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArticleDto {
    pub article_id: u64,
    pub category_id: Option<u64>,
    pub title: String,
    pub summary: Option<String>,
    pub content: String,
    pub author: Option<String>,
    pub source: Option<String>,
    pub tags: Option<String>,
    pub view_count: i64,
    pub like_count: i64,
    pub is_featured: bool,
    pub is_published: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QnaDto {
    pub qna_id: u64,
    pub category_id: Option<u64>,
    pub question: String,
    pub answer: String,
    pub expert_name: Option<String>,
    pub expert_title: Option<String>,
    pub tags: Option<String>,
    pub view_count: i64,
    pub like_count: i64,
    pub is_verified: bool,
    pub is_published: bool,
    pub created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDto {
    pub resource_id: u64,
    pub category_id: Option<u64>,
    pub resource_type: String,
    pub title: String,
    pub description: Option<String>,
    pub object_id: Option<u64>,
    pub external_url: Option<String>,
    pub file_size: Option<u64>,
    pub mime_type: Option<String>,
    pub duration: Option<u32>,
    pub tags: Option<String>,
    pub view_count: i64,
    pub like_count: i64,
    pub is_published: bool,
    pub created_at: String,
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
    pub content_type: Option<String>,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToggleLikeRequest {
    pub content_type: String,
    pub content_id: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LikeStatusDto {
    pub liked: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminContentQuery {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    pub search: Option<String>,
    pub category_id: Option<u64>,
    pub resource_type: Option<String>,
    pub is_verified: Option<bool>,
    pub is_published: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryWriteRequest {
    pub parent_id: Option<u64>,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default = "default_true")]
    pub is_enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArticleWriteRequest {
    pub category_id: u64,
    pub title: String,
    pub summary: Option<String>,
    pub content: String,
    pub author: Option<String>,
    pub source: Option<String>,
    pub tags: Option<serde_json::Value>,
    #[serde(default)]
    pub is_featured: bool,
    #[serde(default = "default_true")]
    pub is_published: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QnaWriteRequest {
    pub category_id: u64,
    pub question: String,
    pub answer: String,
    pub expert_name: Option<String>,
    pub expert_title: Option<String>,
    pub tags: Option<serde_json::Value>,
    #[serde(default)]
    pub is_verified: bool,
    #[serde(default = "default_true")]
    pub is_published: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceWriteRequest {
    pub category_id: u64,
    pub title: String,
    pub description: Option<String>,
    pub resource_type: String,
    pub external_url: Option<String>,
    pub tags: Option<serde_json::Value>,
    #[serde(default = "default_true")]
    pub is_published: bool,
}

#[derive(Serialize)]
pub struct DeletedDto {
    pub deleted: bool,
}

fn default_true() -> bool {
    true
}

fn category_dto(category: crate::domain::psychology::PsychologyCategory) -> CategoryDto {
    CategoryDto {
        category_id: category.id,
        category_name: category.name,
        parent_id: category.parent_id,
        description: category.description,
        sort_order: category.sort_order,
        is_enabled: category.is_enabled,
        children: Vec::new(),
    }
}

fn article_dto(article: crate::domain::psychology::PsychologyArticle) -> ArticleDto {
    ArticleDto {
        article_id: article.id,
        category_id: article.category_id,
        title: article.title,
        summary: article.summary,
        content: article.content,
        author: article.author,
        source: article.source,
        tags: article.tags,
        view_count: article.view_count,
        like_count: article.like_count,
        is_featured: article.is_featured,
        is_published: article.is_published,
        created_at: article.created_at.to_rfc3339(),
        updated_at: article.updated_at.to_rfc3339(),
    }
}

fn qna_dto(qna: crate::domain::psychology::PsychologyQna) -> QnaDto {
    QnaDto {
        qna_id: qna.id,
        category_id: qna.category_id,
        question: qna.question,
        answer: qna.answer,
        expert_name: qna.expert_name,
        expert_title: qna.expert_title,
        tags: qna.tags,
        view_count: qna.view_count,
        like_count: qna.like_count,
        is_verified: qna.is_verified,
        is_published: qna.is_published,
        created_at: qna.created_at.to_rfc3339(),
    }
}

fn resource_dto(resource: crate::domain::psychology::PsychologyResource) -> ResourceDto {
    ResourceDto {
        resource_id: resource.id,
        category_id: resource.category_id,
        resource_type: resource.resource_type,
        title: resource.title,
        description: resource.description,
        object_id: resource.object_id,
        external_url: resource.external_url,
        file_size: resource.file_size,
        mime_type: resource.mime_type,
        duration: resource.duration,
        tags: resource.tags,
        view_count: resource.view_count,
        like_count: resource.like_count,
        is_published: resource.is_published,
        created_at: resource.created_at.to_rfc3339(),
    }
}

// ── Categories ────────────────────────────────────────────────────────────────

pub async fn list_categories(
    State(state): State<PsychologyState>,
) -> Result<Json<Vec<CategoryDto>>, AppError> {
    let categories = state.psychology.list_categories().await?;
    let dtos = categories.into_iter().map(category_dto).collect();
    Ok(Json(dtos))
}

pub async fn get_category_tree(
    State(state): State<PsychologyState>,
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
            description: c.description.clone(),
            sort_order: c.sort_order,
            is_enabled: c.is_enabled,
            children: build_category_tree(all.clone(), Some(c.id)),
        })
        .collect()
}

// ── Articles ──────────────────────────────────────────────────────────────────

pub async fn list_articles(
    State(state): State<PsychologyState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<PaginatedResponse<ArticleDto>>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).max(1).min(100);
    let (articles, total) = state
        .psychology
        .list_articles(page, page_size, None, query.category_id, None)
        .await?;
    let items = articles.into_iter().map(article_dto).collect();
    Ok(Json(PaginatedResponse {
        items,
        page,
        page_size,
        total,
    }))
}

pub async fn get_article(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
) -> Result<Json<ArticleDto>, AppError> {
    let article = state.psychology.get_article(id).await?;
    Ok(Json(article_dto(article)))
}

// ── QnA ───────────────────────────────────────────────────────────────────────

pub async fn list_qna(
    State(state): State<PsychologyState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<PaginatedResponse<QnaDto>>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).max(1).min(100);
    let (qnas, total) = state
        .psychology
        .list_qnas(page, page_size, query.category_id, None)
        .await?;
    let items = qnas.into_iter().map(qna_dto).collect();
    Ok(Json(PaginatedResponse {
        items,
        page,
        page_size,
        total,
    }))
}

pub async fn get_qna(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
) -> Result<Json<QnaDto>, AppError> {
    let qna = state.psychology.get_qna(id).await?;
    Ok(Json(qna_dto(qna)))
}

// ── Resources ─────────────────────────────────────────────────────────────────

pub async fn list_resources(
    State(state): State<PsychologyState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<PaginatedResponse<ResourceDto>>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).max(1).min(100);
    let (resources, total) = state
        .psychology
        .list_resources(page, page_size, query.category_id, None)
        .await?;
    let items = resources.into_iter().map(resource_dto).collect();
    Ok(Json(PaginatedResponse {
        items,
        page,
        page_size,
        total,
    }))
}

pub async fn get_resource(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
) -> Result<Json<ResourceDto>, AppError> {
    let resource = state.psychology.get_resource(id).await?;
    Ok(Json(resource_dto(resource)))
}

// ── Admin content management ─────────────────────────────────────────────────

pub async fn admin_list_categories(
    State(state): State<PsychologyState>,
) -> Result<Json<Vec<CategoryDto>>, AppError> {
    let items = state
        .psychology
        .admin_list_categories()
        .await?
        .into_iter()
        .map(category_dto)
        .collect();
    Ok(Json(items))
}

pub async fn admin_get_category(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
) -> Result<Json<CategoryDto>, AppError> {
    Ok(Json(category_dto(state.psychology.get_category(id).await?)))
}

pub async fn admin_create_category(
    State(state): State<PsychologyState>,
    Json(payload): Json<CategoryWriteRequest>,
) -> Result<Json<CategoryDto>, AppError> {
    let category = state
        .psychology
        .create_category(
            true,
            NewPsychologyCategory {
                parent_id: payload.parent_id,
                name: payload.name,
                description: payload.description,
                sort_order: payload.sort_order,
                is_enabled: payload.is_enabled,
            },
        )
        .await?;
    Ok(Json(category_dto(category)))
}

pub async fn admin_update_category(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
    Json(payload): Json<CategoryWriteRequest>,
) -> Result<Json<CategoryDto>, AppError> {
    let category = state
        .psychology
        .update_category(
            true,
            id,
            NewPsychologyCategory {
                parent_id: payload.parent_id,
                name: payload.name,
                description: payload.description,
                sort_order: payload.sort_order,
                is_enabled: payload.is_enabled,
            },
        )
        .await?;
    Ok(Json(category_dto(category)))
}

pub async fn admin_delete_category(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
) -> Result<Json<DeletedDto>, AppError> {
    state.psychology.delete_category(true, id).await?;
    Ok(Json(DeletedDto { deleted: true }))
}

pub async fn admin_list_articles(
    State(state): State<PsychologyState>,
    Query(query): Query<AdminContentQuery>,
) -> Result<Json<PaginatedResponse<ArticleDto>>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let (articles, total) = state
        .psychology
        .admin_list_articles(
            page,
            page_size,
            query.search,
            query.category_id,
            query.is_published,
        )
        .await?;
    Ok(Json(PaginatedResponse {
        items: articles.into_iter().map(article_dto).collect(),
        page,
        page_size,
        total,
    }))
}

pub async fn admin_get_article(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
) -> Result<Json<ArticleDto>, AppError> {
    Ok(Json(article_dto(
        state.psychology.admin_get_article(id).await?,
    )))
}

pub async fn admin_create_article(
    State(state): State<PsychologyState>,
    Json(payload): Json<ArticleWriteRequest>,
) -> Result<Json<ArticleDto>, AppError> {
    let article = state
        .psychology
        .create_article(
            true,
            NewPsychologyArticle {
                category_id: Some(payload.category_id),
                title: payload.title,
                summary: payload.summary,
                content: payload.content,
                author: payload.author,
                source: payload.source,
                tags: payload.tags.map(|tags| tags.to_string()),
                is_featured: payload.is_featured,
                is_published: payload.is_published,
            },
        )
        .await?;
    Ok(Json(article_dto(article)))
}

pub async fn admin_update_article(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
    Json(payload): Json<ArticleWriteRequest>,
) -> Result<Json<ArticleDto>, AppError> {
    let article = state
        .psychology
        .update_article(
            true,
            id,
            NewPsychologyArticle {
                category_id: Some(payload.category_id),
                title: payload.title,
                summary: payload.summary,
                content: payload.content,
                author: payload.author,
                source: payload.source,
                tags: payload.tags.map(|tags| tags.to_string()),
                is_featured: payload.is_featured,
                is_published: payload.is_published,
            },
        )
        .await?;
    Ok(Json(article_dto(article)))
}

pub async fn admin_delete_article(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
) -> Result<Json<DeletedDto>, AppError> {
    state.psychology.delete_article(true, id).await?;
    Ok(Json(DeletedDto { deleted: true }))
}

pub async fn admin_list_qna(
    State(state): State<PsychologyState>,
    Query(query): Query<AdminContentQuery>,
) -> Result<Json<PaginatedResponse<QnaDto>>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let (qnas, total) = state
        .psychology
        .admin_list_qnas(
            page,
            page_size,
            query.category_id,
            query.is_verified,
            query.is_published,
        )
        .await?;
    Ok(Json(PaginatedResponse {
        items: qnas.into_iter().map(qna_dto).collect(),
        page,
        page_size,
        total,
    }))
}

pub async fn admin_get_qna(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
) -> Result<Json<QnaDto>, AppError> {
    Ok(Json(qna_dto(state.psychology.admin_get_qna(id).await?)))
}

pub async fn admin_create_qna(
    State(state): State<PsychologyState>,
    Json(payload): Json<QnaWriteRequest>,
) -> Result<Json<QnaDto>, AppError> {
    let qna = state
        .psychology
        .create_qna(
            true,
            NewPsychologyQna {
                category_id: Some(payload.category_id),
                question: payload.question,
                answer: payload.answer,
                expert_name: payload.expert_name,
                expert_title: payload.expert_title,
                tags: payload.tags.map(|tags| tags.to_string()),
                is_verified: payload.is_verified,
                is_published: payload.is_published,
            },
        )
        .await?;
    Ok(Json(qna_dto(qna)))
}

pub async fn admin_update_qna(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
    Json(payload): Json<QnaWriteRequest>,
) -> Result<Json<QnaDto>, AppError> {
    let qna = state
        .psychology
        .update_qna(
            true,
            id,
            NewPsychologyQna {
                category_id: Some(payload.category_id),
                question: payload.question,
                answer: payload.answer,
                expert_name: payload.expert_name,
                expert_title: payload.expert_title,
                tags: payload.tags.map(|tags| tags.to_string()),
                is_verified: payload.is_verified,
                is_published: payload.is_published,
            },
        )
        .await?;
    Ok(Json(qna_dto(qna)))
}

pub async fn admin_delete_qna(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
) -> Result<Json<DeletedDto>, AppError> {
    state.psychology.delete_qna(true, id).await?;
    Ok(Json(DeletedDto { deleted: true }))
}

pub async fn admin_list_resources(
    State(state): State<PsychologyState>,
    Query(query): Query<AdminContentQuery>,
) -> Result<Json<PaginatedResponse<ResourceDto>>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let (resources, total) = state
        .psychology
        .admin_list_resources(
            page,
            page_size,
            query.category_id,
            query.resource_type,
            query.is_published,
        )
        .await?;
    Ok(Json(PaginatedResponse {
        items: resources.into_iter().map(resource_dto).collect(),
        page,
        page_size,
        total,
    }))
}

pub async fn admin_get_resource(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
) -> Result<Json<ResourceDto>, AppError> {
    Ok(Json(resource_dto(
        state.psychology.admin_get_resource(id).await?,
    )))
}

pub async fn admin_create_resource(
    State(state): State<PsychologyState>,
    Json(payload): Json<ResourceWriteRequest>,
) -> Result<Json<ResourceDto>, AppError> {
    let resource = state
        .psychology
        .create_resource(
            true,
            NewPsychologyResource {
                category_id: Some(payload.category_id),
                title: payload.title,
                description: payload.description,
                resource_type: payload.resource_type,
                object_id: None,
                external_url: payload.external_url,
                tags: payload.tags.map(|tags| tags.to_string()),
                is_published: payload.is_published,
            },
        )
        .await?;
    Ok(Json(resource_dto(resource)))
}

pub async fn admin_update_resource(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
    Json(payload): Json<ResourceWriteRequest>,
) -> Result<Json<ResourceDto>, AppError> {
    let resource = state
        .psychology
        .update_resource(
            true,
            id,
            NewPsychologyResource {
                category_id: Some(payload.category_id),
                title: payload.title,
                description: payload.description,
                resource_type: payload.resource_type,
                object_id: None,
                external_url: payload.external_url,
                tags: payload.tags.map(|tags| tags.to_string()),
                is_published: payload.is_published,
            },
        )
        .await?;
    Ok(Json(resource_dto(resource)))
}

pub async fn admin_delete_resource(
    State(state): State<PsychologyState>,
    Path(id): Path<u64>,
) -> Result<Json<DeletedDto>, AppError> {
    state.psychology.delete_resource(true, id).await?;
    Ok(Json(DeletedDto { deleted: true }))
}

// ── Favorites ─────────────────────────────────────────────────────────────────

pub async fn toggle_favorite(
    State(state): State<PsychologyState>,
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
    State(state): State<PsychologyState>,
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
    State(state): State<PsychologyState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Query(query): Query<PageQuery>,
) -> Result<Json<PaginatedResponse<FavoriteDto>>, AppError> {
    let content_type = query.content_type.clone();
    let favorites = state
        .psychology
        .list_favorites(auth_user.user_id, content_type.as_deref())
        .await?;
    let total = favorites.len() as u64;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let offset = ((page - 1) * page_size) as usize;
    let items: Vec<FavoriteDto> = favorites
        .into_iter()
        .skip(offset)
        .take(page_size as usize)
        .map(|f| FavoriteDto {
            id: f.id,
            content_type: f.content_type,
            content_id: f.content_id,
        })
        .collect();
    Ok(Json(PaginatedResponse {
        total,
        page,
        page_size,
        items,
    }))
}

// ── Likes ─────────────────────────────────────────────────────────────────────

pub async fn toggle_like(
    State(state): State<PsychologyState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Json(payload): Json<ToggleLikeRequest>,
) -> Result<Json<LikeStatusDto>, AppError> {
    let liked = state
        .psychology
        .toggle_like(
            Some(auth_user.user_id),
            payload.content_type,
            payload.content_id,
        )
        .await?;
    Ok(Json(LikeStatusDto { liked }))
}
