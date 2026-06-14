use std::sync::Arc;

use crate::domain::community::{
    ArticleStatus, Comment, CommunityRepository, NewComment, NewPost, Post, PostUpdate,
};
use crate::shared::error::AppError;

pub struct CommunityService {
    repo: Arc<dyn CommunityRepository>,
}

impl CommunityService {
    pub fn new(repo: Arc<dyn CommunityRepository>) -> Self {
        Self { repo }
    }

    pub async fn list_posts(
        &self,
        page: u64,
        page_size: u64,
    ) -> Result<(Vec<Post>, u64), AppError> {
        let offset = (page.saturating_sub(1)) * page_size;
        let items = self.repo.list_posts(page_size, offset).await?;
        let total = self.repo.count_posts().await?;
        Ok((items, total))
    }

    pub async fn get_post(&self, post_id: u64) -> Result<Post, AppError> {
        let post = self
            .repo
            .find_post_by_id(post_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("post {post_id} not found")))?;
        if !post.is_published() {
            return Err(AppError::NotFound(format!("post {post_id} not found")));
        }
        Ok(post)
    }

    pub async fn create_post(
        &self,
        user_id: u64,
        title: Option<String>,
        content: String,
    ) -> Result<Post, AppError> {
        if content.trim().is_empty() {
            return Err(AppError::Validation("post content cannot be empty".into()));
        }
        let new_post = NewPost::new(user_id, title, content, None, ArticleStatus::Published);
        self.repo.save_post(new_post).await
    }

    pub async fn update_post(
        &self,
        post_id: u64,
        user_id: u64,
        title: Option<String>,
        content: Option<String>,
    ) -> Result<Post, AppError> {
        if title.is_none() && content.is_none() {
            return Err(AppError::Validation(
                "at least one post field must be provided".into(),
            ));
        }
        if content
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(AppError::Validation("post content cannot be empty".into()));
        }
        let existing = self
            .repo
            .find_post_by_id(post_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("post {post_id} not found")))?;
        if existing.user_id != user_id {
            return Err(AppError::Forbidden("you do not own this post".to_string()));
        }
        let update = PostUpdate {
            title: title.map(Some),
            content,
            extra_metadata: None,
            status: None,
        };
        self.repo.update_post(post_id, update).await
    }

    pub async fn delete_post(&self, post_id: u64, user_id: u64) -> Result<bool, AppError> {
        let existing = self
            .repo
            .find_post_by_id(post_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("post {post_id} not found")))?;
        if existing.user_id != user_id {
            return Err(AppError::Forbidden("you do not own this post".to_string()));
        }
        self.repo.delete_post(post_id).await
    }

    pub async fn list_comments(
        &self,
        post_id: u64,
        page: u64,
        page_size: u64,
    ) -> Result<(Vec<Comment>, u64), AppError> {
        let offset = (page.saturating_sub(1)) * page_size;
        let items = self
            .repo
            .list_comments_by_post(post_id, page_size, offset)
            .await?;
        let total = self.repo.count_comments_by_post(post_id).await?;
        Ok((items, total))
    }

    pub async fn create_comment(
        &self,
        post_id: u64,
        user_id: u64,
        content: String,
        parent_comment_id: Option<u64>,
    ) -> Result<Comment, AppError> {
        if content.trim().is_empty() {
            return Err(AppError::Validation(
                "comment content cannot be empty".into(),
            ));
        }
        // verify the post exists
        let post = self
            .repo
            .find_post_by_id(post_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("post {post_id} not found")))?;
        if !post.is_published() {
            return Err(AppError::NotFound(format!("post {post_id} not found")));
        }

        if let Some(parent_id) = parent_comment_id {
            let parent = self
                .repo
                .find_comment_by_id(parent_id)
                .await?
                .ok_or_else(|| {
                    AppError::NotFound(format!("parent comment {parent_id} not found"))
                })?;
            if parent.post_id != post_id {
                return Err(AppError::Validation(
                    "parent comment belongs to a different post".into(),
                ));
            }
        }

        let new_comment = NewComment::new(
            post_id,
            user_id,
            parent_comment_id,
            content,
            None,
            ArticleStatus::Published,
        );
        self.repo.save_comment(new_comment).await
    }

    pub async fn delete_comment(
        &self,
        post_id: u64,
        comment_id: u64,
        user_id: u64,
    ) -> Result<bool, AppError> {
        let existing = self
            .repo
            .find_comment_by_id(comment_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("comment {comment_id} not found")))?;
        if existing.user_id != user_id {
            return Err(AppError::Forbidden(
                "you do not own this comment".to_string(),
            ));
        }
        if existing.post_id != post_id {
            return Err(AppError::NotFound(format!(
                "comment {comment_id} not found in post {post_id}"
            )));
        }
        self.repo.delete_comment(comment_id).await
    }

    // ── Like / Unlike ────────────────────────────────────────────────────────

    pub async fn like_post(&self, post_id: u64, user_id: u64) -> Result<(), AppError> {
        self.repo
            .find_post_by_id(post_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("post {post_id} not found")))?;
        self.repo.like_post(post_id, user_id).await
    }

    pub async fn unlike_post(&self, post_id: u64, user_id: u64) -> Result<(), AppError> {
        self.repo
            .find_post_by_id(post_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("post {post_id} not found")))?;
        self.repo.unlike_post(post_id, user_id).await
    }

    pub async fn like_comment(
        &self,
        post_id: u64,
        comment_id: u64,
        user_id: u64,
    ) -> Result<(), AppError> {
        let comment = self
            .repo
            .find_comment_by_id(comment_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("comment {comment_id} not found")))?;
        if comment.post_id != post_id {
            return Err(AppError::NotFound(format!(
                "comment {comment_id} not found in post {post_id}"
            )));
        }
        self.repo.like_comment(comment_id, user_id).await
    }

    pub async fn unlike_comment(
        &self,
        post_id: u64,
        comment_id: u64,
        user_id: u64,
    ) -> Result<(), AppError> {
        let comment = self
            .repo
            .find_comment_by_id(comment_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("comment {comment_id} not found")))?;
        if comment.post_id != post_id {
            return Err(AppError::NotFound(format!(
                "comment {comment_id} not found in post {post_id}"
            )));
        }
        self.repo.unlike_comment(comment_id, user_id).await
    }
}
