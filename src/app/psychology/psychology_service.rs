use std::sync::Arc;

use crate::domain::psychology::{
    KnowledgeFavorite, NewContentLike, NewKnowledgeFavorite, NewPsychologyArticle,
    NewPsychologyCategory, NewPsychologyQna, NewPsychologyResource, PsychologyArticle,
    PsychologyCategory, PsychologyQna, PsychologyRepoT, PsychologyResource,
};
use crate::shared::error::AppError;

pub struct PsychologyService {
    repo: Arc<dyn PsychologyRepoT>,
}

impl PsychologyService {
    pub fn new(repo: Arc<dyn PsychologyRepoT>) -> Self {
        Self { repo }
    }

    // ── Categories (admin write, public read) ──

    pub async fn list_categories(&self) -> Result<Vec<PsychologyCategory>, AppError> {
        self.repo.list_categories().await
    }

    pub async fn admin_list_categories(&self) -> Result<Vec<PsychologyCategory>, AppError> {
        self.repo.list_categories_admin().await
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
        mut new: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        new.name = new.name.trim().to_string();
        if new.name.is_empty() || new.name.chars().count() > 50 {
            return Err(AppError::Validation(
                "category name must contain 1 to 50 characters".into(),
            ));
        }
        self.validate_category_parent(None, new.parent_id).await?;
        self.repo.create_category(new).await
    }

    pub async fn update_category(
        &self,
        is_admin: bool,
        id: u64,
        mut new: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        new.name = new.name.trim().to_string();
        if new.name.is_empty() || new.name.chars().count() > 50 {
            return Err(AppError::Validation(
                "category name must contain 1 to 50 characters".into(),
            ));
        }
        self.validate_category_parent(Some(id), new.parent_id)
            .await?;
        self.repo.update_category(id, new).await
    }

    pub async fn delete_category(&self, is_admin: bool, id: u64) -> Result<bool, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        let deleted = self.repo.delete_category(id).await?;
        if !deleted {
            return Err(AppError::NotFound("category not found".into()));
        }
        Ok(true)
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
        self.repo
            .list_articles(page, page_size, search, category_id, is_featured)
            .await
    }

    pub async fn get_article(&self, id: u64) -> Result<PsychologyArticle, AppError> {
        let article = self
            .repo
            .find_article_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("article not found".into()))?;
        if !article.is_published {
            return Err(AppError::NotFound("article not found".into()));
        }
        Ok(article)
    }

    pub async fn admin_list_articles(
        &self,
        page: u64,
        page_size: u64,
        search: Option<String>,
        category_id: Option<u64>,
        is_published: Option<bool>,
    ) -> Result<(Vec<PsychologyArticle>, u64), AppError> {
        self.repo
            .list_articles_admin(
                page.max(1),
                page_size.clamp(1, 100),
                search,
                category_id,
                is_published,
            )
            .await
    }

    pub async fn admin_get_article(&self, id: u64) -> Result<PsychologyArticle, AppError> {
        self.repo
            .find_article_by_id_admin(id)
            .await?
            .ok_or_else(|| AppError::NotFound("article not found".into()))
    }

    pub async fn create_article(
        &self,
        is_admin: bool,
        mut new: NewPsychologyArticle,
    ) -> Result<PsychologyArticle, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.validate_article(&mut new).await?;
        self.repo.create_article(new).await
    }

    pub async fn update_article(
        &self,
        is_admin: bool,
        id: u64,
        mut new: NewPsychologyArticle,
    ) -> Result<PsychologyArticle, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.validate_article(&mut new).await?;
        self.repo.update_article(id, new).await
    }

    pub async fn delete_article(&self, is_admin: bool, id: u64) -> Result<bool, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        let deleted = self.repo.delete_article(id).await?;
        if !deleted {
            return Err(AppError::NotFound("article not found".into()));
        }
        Ok(true)
    }

    // ── QnA ──

    pub async fn list_qnas(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        is_verified: Option<bool>,
    ) -> Result<(Vec<PsychologyQna>, u64), AppError> {
        self.repo
            .list_qnas(page, page_size, category_id, is_verified)
            .await
    }

    pub async fn get_qna(&self, id: u64) -> Result<PsychologyQna, AppError> {
        let qna = self
            .repo
            .find_qna_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("qna not found".into()))?;
        if !qna.is_published {
            return Err(AppError::NotFound("qna not found".into()));
        }
        Ok(qna)
    }

    pub async fn admin_list_qnas(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        is_verified: Option<bool>,
        is_published: Option<bool>,
    ) -> Result<(Vec<PsychologyQna>, u64), AppError> {
        self.repo
            .list_qnas_admin(
                page.max(1),
                page_size.clamp(1, 100),
                category_id,
                is_verified,
                is_published,
            )
            .await
    }

    pub async fn admin_get_qna(&self, id: u64) -> Result<PsychologyQna, AppError> {
        self.repo
            .find_qna_by_id_admin(id)
            .await?
            .ok_or_else(|| AppError::NotFound("qna not found".into()))
    }

    pub async fn create_qna(
        &self,
        is_admin: bool,
        mut new: NewPsychologyQna,
    ) -> Result<PsychologyQna, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.validate_qna(&mut new).await?;
        self.repo.create_qna(new).await
    }

    pub async fn update_qna(
        &self,
        is_admin: bool,
        id: u64,
        mut new: NewPsychologyQna,
    ) -> Result<PsychologyQna, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.validate_qna(&mut new).await?;
        self.repo.update_qna(id, new).await
    }

    pub async fn delete_qna(&self, is_admin: bool, id: u64) -> Result<bool, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        let deleted = self.repo.delete_qna(id).await?;
        if !deleted {
            return Err(AppError::NotFound("qna not found".into()));
        }
        Ok(true)
    }

    // ── Resources ──

    pub async fn list_resources(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        resource_type: Option<String>,
    ) -> Result<(Vec<PsychologyResource>, u64), AppError> {
        self.repo
            .list_resources(page, page_size, category_id, resource_type)
            .await
    }

    pub async fn get_resource(&self, id: u64) -> Result<PsychologyResource, AppError> {
        let resource = self
            .repo
            .find_resource_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("resource not found".into()))?;
        if !resource.is_published {
            return Err(AppError::NotFound("resource not found".into()));
        }
        Ok(resource)
    }

    pub async fn admin_list_resources(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        resource_type: Option<String>,
        is_published: Option<bool>,
    ) -> Result<(Vec<PsychologyResource>, u64), AppError> {
        self.repo
            .list_resources_admin(
                page.max(1),
                page_size.clamp(1, 100),
                category_id,
                resource_type,
                is_published,
            )
            .await
    }

    pub async fn admin_get_resource(&self, id: u64) -> Result<PsychologyResource, AppError> {
        self.repo
            .find_resource_by_id_admin(id)
            .await?
            .ok_or_else(|| AppError::NotFound("resource not found".into()))
    }

    pub async fn create_resource(
        &self,
        is_admin: bool,
        mut new: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.validate_resource(&mut new).await?;
        self.repo.create_resource(new).await
    }

    pub async fn update_resource(
        &self,
        is_admin: bool,
        id: u64,
        mut new: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        self.validate_resource(&mut new).await?;
        self.repo.update_resource(id, new).await
    }

    pub async fn delete_resource(&self, is_admin: bool, id: u64) -> Result<bool, AppError> {
        if !is_admin {
            return Err(AppError::Forbidden("admin only".into()));
        }
        let deleted = self.repo.delete_resource(id).await?;
        if !deleted {
            return Err(AppError::NotFound("resource not found".into()));
        }
        Ok(true)
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

    async fn validate_article(&self, new: &mut NewPsychologyArticle) -> Result<(), AppError> {
        self.validate_category(new.category_id).await?;
        new.title = new.title.trim().to_string();
        new.content = new.content.trim().to_string();
        new.author = Self::trim_optional(new.author.take());
        new.source = Self::trim_optional(new.source.take());
        if new.title.is_empty() || new.title.chars().count() > 200 {
            return Err(AppError::Validation(
                "article title must contain 1 to 200 characters".into(),
            ));
        }
        if new.content.is_empty() {
            return Err(AppError::Validation(
                "article content cannot be empty".into(),
            ));
        }
        Self::normalize_tags(&mut new.tags)
    }

    async fn validate_qna(&self, new: &mut NewPsychologyQna) -> Result<(), AppError> {
        self.validate_category(new.category_id).await?;
        new.question = new.question.trim().to_string();
        new.answer = new.answer.trim().to_string();
        new.expert_name = Self::trim_optional(new.expert_name.take());
        new.expert_title = Self::trim_optional(new.expert_title.take());
        if new.question.is_empty() || new.answer.is_empty() {
            return Err(AppError::Validation(
                "question and answer cannot be empty".into(),
            ));
        }
        Self::normalize_tags(&mut new.tags)
    }

    async fn validate_resource(&self, new: &mut NewPsychologyResource) -> Result<(), AppError> {
        self.validate_category(new.category_id).await?;
        new.title = new.title.trim().to_string();
        new.resource_type = new.resource_type.trim().to_ascii_uppercase();
        if new.title.is_empty() || new.title.chars().count() > 200 {
            return Err(AppError::Validation(
                "resource title must contain 1 to 200 characters".into(),
            ));
        }
        if !matches!(
            new.resource_type.as_str(),
            "VIDEO" | "AUDIO" | "PDF" | "LINK" | "TOOL"
        ) {
            return Err(AppError::Validation(
                "resource_type must be VIDEO, AUDIO, PDF, LINK, or TOOL".into(),
            ));
        }
        if let Some(url) = new.external_url.as_mut() {
            *url = url.trim().to_string();
            let parsed = reqwest::Url::parse(url)
                .map_err(|_| AppError::Validation("external_url is invalid".into()))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(AppError::Validation(
                    "external_url must use http or https".into(),
                ));
            }
        }
        Self::normalize_tags(&mut new.tags)
    }

    async fn validate_category(&self, category_id: Option<u64>) -> Result<(), AppError> {
        let category_id =
            category_id.ok_or_else(|| AppError::Validation("category_id is required".into()))?;
        self.get_category(category_id).await?;
        Ok(())
    }

    async fn validate_category_parent(
        &self,
        category_id: Option<u64>,
        parent_id: Option<u64>,
    ) -> Result<(), AppError> {
        let mut current = parent_id;
        let mut visited = std::collections::HashSet::new();
        while let Some(id) = current {
            if Some(id) == category_id || !visited.insert(id) {
                return Err(AppError::Validation(
                    "category parent relationship would create a cycle".into(),
                ));
            }
            current = self.get_category(id).await?.parent_id;
        }
        Ok(())
    }

    fn trim_optional(value: Option<String>) -> Option<String> {
        value
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
    }

    fn normalize_tags(tags: &mut Option<String>) -> Result<(), AppError> {
        if let Some(raw) = tags {
            let parsed: serde_json::Value = serde_json::from_str(raw)
                .map_err(|_| AppError::Validation("tags must be a JSON array".into()))?;
            if !parsed.is_array() {
                return Err(AppError::Validation("tags must be a JSON array".into()));
            }
            *raw = serde_json::to_string(&parsed)
                .map_err(|e| AppError::Internal(format!("failed to normalize tags: {e}")))?;
        }
        Ok(())
    }
}
