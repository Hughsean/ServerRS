use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::shared::error::AppError;

#[derive(Debug, Clone)]
pub struct PsychologyCategory {
    pub id: u64,
    pub parent_id: Option<u64>,
    pub name: String,
    pub description: Option<String>,
    pub sort_order: i32,
    pub is_enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewPsychologyCategory {
    pub parent_id: Option<u64>,
    pub name: String,
    pub description: Option<String>,
    pub sort_order: i32,
    pub is_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct PsychologyArticle {
    pub id: u64,
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewPsychologyArticle {
    pub category_id: Option<u64>,
    pub title: String,
    pub summary: Option<String>,
    pub content: String,
    pub author: Option<String>,
    pub source: Option<String>,
    pub tags: Option<String>,
    pub is_featured: bool,
    pub is_published: bool,
}

#[derive(Debug, Clone)]
pub struct PsychologyQna {
    pub id: u64,
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
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewPsychologyQna {
    pub category_id: Option<u64>,
    pub question: String,
    pub answer: String,
    pub expert_name: Option<String>,
    pub expert_title: Option<String>,
    pub tags: Option<String>,
    pub is_verified: bool,
    pub is_published: bool,
}

#[derive(Debug, Clone)]
pub struct PsychologyResource {
    pub id: u64,
    pub category_id: Option<u64>,
    pub title: String,
    pub description: Option<String>,
    pub resource_type: String,
    pub object_id: Option<u64>,
    pub external_url: Option<String>,
    pub file_size: Option<u64>,
    pub mime_type: Option<String>,
    pub duration: Option<u32>,
    pub tags: Option<String>,
    pub view_count: i64,
    pub like_count: i64,
    pub is_published: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewPsychologyResource {
    pub category_id: Option<u64>,
    pub title: String,
    pub description: Option<String>,
    pub resource_type: String,
    pub object_id: Option<u64>,
    pub external_url: Option<String>,
    pub tags: Option<String>,
    pub is_published: bool,
}

#[derive(Debug, Clone)]
pub struct KnowledgeFavorite {
    pub id: u64,
    pub user_id: u64,
    pub content_type: String,
    pub content_id: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewKnowledgeFavorite {
    pub user_id: u64,
    pub content_type: String,
    pub content_id: u64,
}

#[derive(Debug, Clone)]
pub struct ContentLike {
    pub id: u64,
    pub user_id: u64,
    pub content_type: String,
    pub content_id: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewContentLike {
    pub user_id: u64,
    pub content_type: String,
    pub content_id: u64,
}

#[async_trait]
pub trait PsychologyRepoT: Send + Sync {
    // Categories
    async fn find_category_by_id(&self, id: u64) -> Result<Option<PsychologyCategory>, AppError>;
    async fn list_categories(&self) -> Result<Vec<PsychologyCategory>, AppError>;
    async fn list_categories_admin(&self) -> Result<Vec<PsychologyCategory>, AppError>;
    async fn create_category(
        &self,
        new: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError>;
    async fn update_category(
        &self,
        id: u64,
        new: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError>;
    async fn delete_category(&self, id: u64) -> Result<bool, AppError>;

    // Articles
    async fn find_article_by_id(&self, id: u64) -> Result<Option<PsychologyArticle>, AppError>;
    async fn list_articles(
        &self,
        page: u64,
        page_size: u64,
        search: Option<String>,
        category_id: Option<u64>,
        is_featured: Option<bool>,
    ) -> Result<(Vec<PsychologyArticle>, u64), AppError>;
    async fn find_article_by_id_admin(
        &self,
        id: u64,
    ) -> Result<Option<PsychologyArticle>, AppError>;
    async fn list_articles_admin(
        &self,
        page: u64,
        page_size: u64,
        search: Option<String>,
        category_id: Option<u64>,
        is_published: Option<bool>,
    ) -> Result<(Vec<PsychologyArticle>, u64), AppError>;
    async fn create_article(
        &self,
        new: NewPsychologyArticle,
    ) -> Result<PsychologyArticle, AppError>;
    async fn update_article(
        &self,
        id: u64,
        new: NewPsychologyArticle,
    ) -> Result<PsychologyArticle, AppError>;
    async fn delete_article(&self, id: u64) -> Result<bool, AppError>;

    // QnA
    async fn find_qna_by_id(&self, id: u64) -> Result<Option<PsychologyQna>, AppError>;
    async fn list_qnas(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        is_verified: Option<bool>,
    ) -> Result<(Vec<PsychologyQna>, u64), AppError>;
    async fn find_qna_by_id_admin(&self, id: u64) -> Result<Option<PsychologyQna>, AppError>;
    async fn list_qnas_admin(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        is_verified: Option<bool>,
        is_published: Option<bool>,
    ) -> Result<(Vec<PsychologyQna>, u64), AppError>;
    async fn create_qna(&self, new: NewPsychologyQna) -> Result<PsychologyQna, AppError>;
    async fn update_qna(&self, id: u64, new: NewPsychologyQna) -> Result<PsychologyQna, AppError>;
    async fn delete_qna(&self, id: u64) -> Result<bool, AppError>;

    // Resources
    async fn find_resource_by_id(&self, id: u64) -> Result<Option<PsychologyResource>, AppError>;
    async fn list_resources(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        resource_type: Option<String>,
    ) -> Result<(Vec<PsychologyResource>, u64), AppError>;
    async fn find_resource_by_id_admin(
        &self,
        id: u64,
    ) -> Result<Option<PsychologyResource>, AppError>;
    async fn list_resources_admin(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        resource_type: Option<String>,
        is_published: Option<bool>,
    ) -> Result<(Vec<PsychologyResource>, u64), AppError>;
    async fn create_resource(
        &self,
        new: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError>;
    async fn update_resource(
        &self,
        id: u64,
        new: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError>;
    async fn delete_resource(&self, id: u64) -> Result<bool, AppError>;

    // Favorites
    async fn toggle_favorite(&self, new: NewKnowledgeFavorite) -> Result<bool, AppError>;
    async fn check_favorite(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError>;
    async fn list_favorites(
        &self,
        user_id: u64,
        content_type: Option<&str>,
    ) -> Result<Vec<KnowledgeFavorite>, AppError>;

    // Likes
    async fn toggle_like(&self, new: NewContentLike) -> Result<bool, AppError>;
}
