use std::sync::Arc;

use crate::domain::psychology::{
    KnowledgeFavorite, NewContentLike, NewKnowledgeFavorite, NewPsychologyArticle,
    NewPsychologyCategory, NewPsychologyQna, NewPsychologyResource, PsychologyArticle,
    PsychologyCategory, PsychologyQna, PsychologyRepository, PsychologyResource,
};
use crate::shared::error::AppError;

pub struct PsychologyService {
    repo: Arc<dyn PsychologyRepository>,
}

impl PsychologyService {
    pub fn new(repo: Arc<dyn PsychologyRepository>) -> Self {
        Self { repo }
    }

    // ── Categories (admin write, public read) ──

    pub async fn list_categories(&self) -> Result<Vec<PsychologyCategory>, AppError> {
        self.repo.list_categories().await
    }

    pub async fn get_category(&self, id: u64) -> Result<PsychologyCategory, AppError> {
        self.repo
            .find_category_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("category not found".into()))
    }

    pub async fn create_category(
        &self,
        is_admin: bool,
        new: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.create_category(new).await
    }

    pub async fn update_category(
        &self,
        is_admin: bool,
        id: u64,
        new: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.update_category(id, new).await
    }

    pub async fn delete_category(&self, is_admin: bool, id: u64) -> Result<bool, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.delete_category(id).await
    }

    // ── Articles ──

    pub async fn list_articles(
        &self,
        page: u64,
        page_size: u64,
        search: Option<String>,
        category_id: Option<u64>,
        is_featured: Option<bool>,
    ) -> Result<(Vec<PsychologyArticle>, u64), AppError> {
        self.repo.list_articles(page, page_size, search, category_id, is_featured).await
    }

    pub async fn get_article(&self, id: u64) -> Result<PsychologyArticle, AppError> {
        self.repo
            .find_article_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("article not found".into()))
    }

    pub async fn create_article(
        &self,
        is_admin: bool,
        new: NewPsychologyArticle,
    ) -> Result<PsychologyArticle, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.create_article(new).await
    }

    pub async fn update_article(
        &self,
        is_admin: bool,
        id: u64,
        new: NewPsychologyArticle,
    ) -> Result<PsychologyArticle, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.update_article(id, new).await
    }

    pub async fn delete_article(&self, is_admin: bool, id: u64) -> Result<bool, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.delete_article(id).await
    }

    // ── QnA ──

    pub async fn list_qnas(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        is_verified: Option<bool>,
    ) -> Result<(Vec<PsychologyQna>, u64), AppError> {
        self.repo.list_qnas(page, page_size, category_id, is_verified).await
    }

    pub async fn get_qna(&self, id: u64) -> Result<PsychologyQna, AppError> {
        self.repo
            .find_qna_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("qna not found".into()))
    }

    pub async fn create_qna(
        &self,
        is_admin: bool,
        new: NewPsychologyQna,
    ) -> Result<PsychologyQna, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.create_qna(new).await
    }

    pub async fn update_qna(
        &self,
        is_admin: bool,
        id: u64,
        new: NewPsychologyQna,
    ) -> Result<PsychologyQna, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.update_qna(id, new).await
    }

    pub async fn delete_qna(&self, is_admin: bool, id: u64) -> Result<bool, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.delete_qna(id).await
    }

    // ── Resources ──

    pub async fn list_resources(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        resource_type: Option<String>,
    ) -> Result<(Vec<PsychologyResource>, u64), AppError> {
        self.repo.list_resources(page, page_size, category_id, resource_type).await
    }

    pub async fn get_resource(&self, id: u64) -> Result<PsychologyResource, AppError> {
        self.repo
            .find_resource_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("resource not found".into()))
    }

    pub async fn create_resource(
        &self,
        is_admin: bool,
        new: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.create_resource(new).await
    }

    pub async fn update_resource(
        &self,
        is_admin: bool,
        id: u64,
        new: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.update_resource(id, new).await
    }

    pub async fn delete_resource(&self, is_admin: bool, id: u64) -> Result<bool, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.repo.delete_resource(id).await
    }

    // ── Favorites / Likes (require authenticated user) ──

    pub async fn toggle_favorite(
        &self,
        user_id: Option<u64>,
        content_type: String,
        content_id: u64,
    ) -> Result<bool, AppError> {
        let uid = user_id.ok_or(AppError::Unauthorized)?;
        self.repo
            .toggle_favorite(NewKnowledgeFavorite {
                user_id: uid,
                content_type,
                content_id,
            })
            .await
    }

    pub async fn check_favorite(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError> {
        self.repo
            .check_favorite(user_id, content_type, content_id)
            .await
    }

    pub async fn list_favorites(
        &self,
        user_id: u64,
        content_type: Option<&str>,
    ) -> Result<Vec<KnowledgeFavorite>, AppError> {
        self.repo.list_favorites(user_id, content_type).await
    }

    pub async fn toggle_like(
        &self,
        user_id: Option<u64>,
        content_type: String,
        content_id: u64,
    ) -> Result<bool, AppError> {
        let uid = user_id.ok_or(AppError::Unauthorized)?;
        self.repo
            .toggle_like(NewContentLike {
                user_id: uid,
                content_type,
                content_id,
            })
            .await
    }
}
