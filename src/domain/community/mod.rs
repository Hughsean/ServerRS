use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::JsonValue;
use serde::Serialize;

use crate::shared::error::AppError;

/// Represents the status of a community post or comment.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArticleStatus {
    /// Visible to all users.
    Published,
    /// Hidden / soft-deleted.
    Hidden,
}

impl ArticleStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::Hidden => "hidden",
        }
    }

    pub fn from_i8(v: i8) -> Option<Self> {
        match v {
            1 => Some(Self::Published),
            0 => Some(Self::Hidden),
            _ => None,
        }
    }

    pub fn to_i8(self) -> i8 {
        match self {
            Self::Published => 1,
            Self::Hidden => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Post
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Post {
    pub post_id: u64,
    pub user_id: u64,
    pub title: Option<String>,
    pub content: String,
    pub extra_metadata: Option<JsonValue>,
    pub likes_count: u32,
    pub comments_count: u32,
    pub status: ArticleStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Post {
    pub fn is_published(&self) -> bool {
        matches!(self.status, ArticleStatus::Published)
    }

    pub fn is_hidden(&self) -> bool {
        matches!(self.status, ArticleStatus::Hidden)
    }
}

#[derive(Debug, Clone)]
pub struct NewPost {
    pub user_id: u64,
    pub title: Option<String>,
    pub content: String,
    pub extra_metadata: Option<JsonValue>,
    pub status: ArticleStatus,
}

impl NewPost {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        user_id: u64,
        title: Option<String>,
        content: impl Into<String>,
        extra_metadata: Option<JsonValue>,
        status: ArticleStatus,
    ) -> Self {
        Self {
            user_id,
            title,
            content: content.into(),
            extra_metadata,
            status,
        }
    }
}

/// Partial update payload for a post (all fields optional).
#[derive(Debug, Clone)]
pub struct PostUpdate {
    pub title: Option<Option<String>>,
    pub content: Option<String>,
    pub extra_metadata: Option<Option<JsonValue>>,
    pub status: Option<ArticleStatus>,
}

impl PostUpdate {
    pub fn has_any(&self) -> bool {
        self.title.is_some()
            || self.content.is_some()
            || self.extra_metadata.is_some()
            || self.status.is_some()
    }
}

// ---------------------------------------------------------------------------
// Comment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Comment {
    pub comment_id: u64,
    pub post_id: u64,
    pub user_id: u64,
    pub parent_comment_id: Option<u64>,
    pub content: String,
    pub attachments: Option<JsonValue>,
    pub likes_count: u32,
    pub status: ArticleStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Comment {
    pub fn is_published(&self) -> bool {
        matches!(self.status, ArticleStatus::Published)
    }

    pub fn is_hidden(&self) -> bool {
        matches!(self.status, ArticleStatus::Hidden)
    }
}

#[derive(Debug, Clone)]
pub struct NewComment {
    pub post_id: u64,
    pub user_id: u64,
    pub parent_comment_id: Option<u64>,
    pub content: String,
    pub attachments: Option<JsonValue>,
    pub status: ArticleStatus,
}

impl NewComment {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        post_id: u64,
        user_id: u64,
        parent_comment_id: Option<u64>,
        content: impl Into<String>,
        attachments: Option<JsonValue>,
        status: ArticleStatus,
    ) -> Self {
        Self {
            post_id,
            user_id,
            parent_comment_id,
            content: content.into(),
            attachments,
            status,
        }
    }
}

// ---------------------------------------------------------------------------
// PostMedia
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct PostMedia {
    pub media_id: u64,
    pub post_id: u64,
    pub media_type: String,
    pub mime_type: String,
    pub media_data: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewPostMedia {
    pub post_id: u64,
    pub media_type: String,
    pub mime_type: String,
    pub media_data: String,
}

impl NewPostMedia {
    pub fn new(
        post_id: u64,
        media_type: impl Into<String>,
        mime_type: impl Into<String>,
        media_data: impl Into<String>,
    ) -> Self {
        Self {
            post_id,
            media_type: media_type.into(),
            mime_type: mime_type.into(),
            media_data: media_data.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// CommunityRepository trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait CommunityRepository: Send + Sync {
    // -- Posts ---------------------------------------------------------------

    async fn list_posts(&self, limit: u64, offset: u64) -> Result<Vec<Post>, AppError>;

    async fn count_posts(&self) -> Result<u64, AppError>;

    async fn find_post_by_id(&self, post_id: u64) -> Result<Option<Post>, AppError>;

    async fn save_post(&self, new_post: NewPost) -> Result<Post, AppError>;

    async fn update_post(&self, post_id: u64, update: PostUpdate) -> Result<Post, AppError>;

    async fn delete_post(&self, post_id: u64) -> Result<bool, AppError>;

    /// Atomically increment the `comments_count` on a post by 1.
    async fn incr_comments_count(&self, post_id: u64) -> Result<(), AppError>;

    /// Atomically decrement the `comments_count` on a post by 1 (never below 0).
    async fn decr_comments_count(&self, post_id: u64) -> Result<(), AppError>;

    // -- Comments ------------------------------------------------------------

    async fn list_comments_by_post(
        &self,
        post_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<Vec<Comment>, AppError>;

    async fn count_comments_by_post(&self, post_id: u64) -> Result<u64, AppError>;

    async fn find_comment_by_id(&self, comment_id: u64) -> Result<Option<Comment>, AppError>;

    async fn save_comment(&self, new_comment: NewComment) -> Result<Comment, AppError>;

    /// Update the **content** and/or **status** of a comment.  `attachments` and
    /// `parent_comment_id` are immutable after creation.
    async fn update_comment(
        &self,
        comment_id: u64,
        content: Option<String>,
        status: Option<ArticleStatus>,
    ) -> Result<Comment, AppError>;

    async fn delete_comment(&self, comment_id: u64) -> Result<bool, AppError>;

    // -- Media ---------------------------------------------------------------

    async fn list_media_by_post(&self, post_id: u64) -> Result<Vec<PostMedia>, AppError>;

    async fn save_media(&self, new_media: NewPostMedia) -> Result<PostMedia, AppError>;

    // -- Likes ----------------------------------------------------------------

    /// Record a like on a post by a user. Returns an error if already liked.
    async fn like_post(&self, post_id: u64, user_id: u64) -> Result<(), AppError>;

    /// Remove a like from a post. Returns an error if not liked.
    async fn unlike_post(&self, post_id: u64, user_id: u64) -> Result<(), AppError>;

    /// Record a like on a comment by a user. Returns an error if already liked.
    async fn like_comment(&self, comment_id: u64, user_id: u64) -> Result<(), AppError>;

    /// Remove a like from a comment. Returns an error if not liked.
    async fn unlike_comment(&self, comment_id: u64, user_id: u64) -> Result<(), AppError>;
}
