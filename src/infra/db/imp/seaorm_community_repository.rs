use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};

use crate::domain::community::{
    ArticleStatus, Comment, CommunityRepository, NewComment, NewPost, NewPostMedia, Post,
    PostMedia, PostUpdate,
};
use crate::shared::error::AppError;

use super::super::entities::{
    community_comments, community_post_media, community_posts, content_likes,
};

// ---------------------------------------------------------------------------
// Mapping helpers
// ---------------------------------------------------------------------------

fn map_post(m: community_posts::Model) -> Post {
    Post {
        post_id: m.post_id,
        user_id: m.user_id,
        title: m.title,
        content: m.content,
        extra_metadata: m.extra_metadata.map(|v| v.into()),
        likes_count: m.likes_count,
        comments_count: m.comments_count,
        status: ArticleStatus::from_i8(m.status).unwrap_or(ArticleStatus::Hidden),
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
    }
}

fn map_comment(m: community_comments::Model) -> Comment {
    Comment {
        comment_id: m.comment_id,
        post_id: m.post_id,
        user_id: m.user_id,
        parent_comment_id: m.parent_comment_id,
        content: m.content,
        attachments: m.attachments.map(|v| v.into()),
        likes_count: m.likes_count,
        status: ArticleStatus::from_i8(m.status).unwrap_or(ArticleStatus::Hidden),
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
    }
}

fn map_media(m: community_post_media::Model) -> PostMedia {
    PostMedia {
        media_id: m.media_id,
        post_id: m.post_id,
        media_type: m.media_type,
        mime_type: m.mime_type,
        media_data: m.media_data,
        created_at: m.created_at.and_utc(),
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

// ---------------------------------------------------------------------------
// Repository struct
// ---------------------------------------------------------------------------

pub struct SeaOrmCommunityRepository {
    db: DatabaseConnection,
}

impl SeaOrmCommunityRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl CommunityRepository for SeaOrmCommunityRepository {
    // -- Posts ---------------------------------------------------------------

    async fn list_posts(&self, limit: u64, offset: u64) -> Result<Vec<Post>, AppError> {
        let paginator = community_posts::Entity::find()
            .filter(community_posts::Column::Status.eq(1i8))
            .order_by_desc(community_posts::Column::CreatedAt)
            .paginate(&self.db, limit);
        let page_num = if limit > 0 { offset / limit } else { 0 };
        let items = paginator.fetch_page(page_num).await.map_err(map_err)?;
        Ok(items.into_iter().map(map_post).collect())
    }

    async fn count_posts(&self) -> Result<u64, AppError> {
        community_posts::Entity::find()
            .filter(community_posts::Column::Status.eq(1i8))
            .count(&self.db)
            .await
            .map_err(map_err)
    }

    async fn find_post_by_id(&self, post_id: u64) -> Result<Option<Post>, AppError> {
        community_posts::Entity::find_by_id(post_id)
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|o| o.map(map_post))
    }

    async fn save_post(&self, new_post: NewPost) -> Result<Post, AppError> {
        let now = chrono::Utc::now();
        let am = community_posts::ActiveModel {
            user_id: Set(new_post.user_id),
            title: Set(new_post.title),
            content: Set(new_post.content),
            extra_metadata: Set(new_post.extra_metadata.map(|v| v.into())),
            likes_count: Set(0),
            comments_count: Set(0),
            status: Set(new_post.status.to_i8()),
            created_at: Set(now.naive_utc()),
            updated_at: Set(now.naive_utc()),
            ..Default::default()
        };
        Ok(map_post(am.insert(&self.db).await.map_err(map_err)?))
    }

    async fn update_post(&self, post_id: u64, update: PostUpdate) -> Result<Post, AppError> {
        let existing = community_posts::Entity::find_by_id(post_id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or(AppError::NotFound("post not found".into()))?;
        let mut am: community_posts::ActiveModel = existing.into();
        if let Some(title) = update.title {
            am.title = Set(title);
        }
        if let Some(content) = update.content {
            am.content = Set(content);
        }
        if let Some(status) = update.status {
            am.status = Set(status.to_i8());
        }
        am.updated_at = Set(chrono::Utc::now().naive_utc());
        Ok(map_post(am.update(&self.db).await.map_err(map_err)?))
    }

    async fn delete_post(&self, post_id: u64) -> Result<bool, AppError> {
        Ok(community_posts::Entity::delete_by_id(post_id)
            .exec(&self.db)
            .await
            .map_err(map_err)?
            .rows_affected
            > 0)
    }

    async fn incr_comments_count(&self, post_id: u64) -> Result<(), AppError> {
        let sql = format!(
            "UPDATE community_posts SET comments_count = comments_count + 1 WHERE post_id = {}",
            post_id
        );
        self.db.execute_unprepared(&sql).await.map_err(map_err)?;
        Ok(())
    }

    async fn decr_comments_count(&self, post_id: u64) -> Result<(), AppError> {
        let sql = format!(
            "UPDATE community_posts SET comments_count = GREATEST(comments_count - 1, 0) WHERE post_id = {}",
            post_id
        );
        self.db.execute_unprepared(&sql).await.map_err(map_err)?;
        Ok(())
    }

    // -- Comments ------------------------------------------------------------

    async fn list_comments_by_post(
        &self,
        post_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<Vec<Comment>, AppError> {
        let paginator = community_comments::Entity::find()
            .filter(community_comments::Column::PostId.eq(post_id))
            .filter(community_comments::Column::Status.eq(1_i8))
            .order_by_desc(community_comments::Column::CreatedAt)
            .paginate(&self.db, limit);
        let page_num = if limit > 0 { offset / limit } else { 0 };
        let items = paginator.fetch_page(page_num).await.map_err(map_err)?;
        Ok(items.into_iter().map(map_comment).collect())
    }

    async fn count_comments_by_post(&self, post_id: u64) -> Result<u64, AppError> {
        community_comments::Entity::find()
            .filter(community_comments::Column::PostId.eq(post_id))
            .filter(community_comments::Column::Status.eq(1_i8))
            .count(&self.db)
            .await
            .map_err(map_err)
    }

    async fn find_comment_by_id(&self, comment_id: u64) -> Result<Option<Comment>, AppError> {
        community_comments::Entity::find_by_id(comment_id)
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|o| o.map(map_comment))
    }

    async fn save_comment(&self, new_comment: NewComment) -> Result<Comment, AppError> {
        let now = chrono::Utc::now();
        let post_id = new_comment.post_id;
        let am = community_comments::ActiveModel {
            post_id: Set(post_id),
            user_id: Set(new_comment.user_id),
            parent_comment_id: Set(new_comment.parent_comment_id),
            content: Set(new_comment.content),
            attachments: Set(new_comment.attachments.map(|v| v.into())),
            likes_count: Set(0),
            status: Set(new_comment.status.to_i8()),
            created_at: Set(now.naive_utc()),
            updated_at: Set(now.naive_utc()),
            ..Default::default()
        };
        let txn = self.db.begin().await.map_err(map_err)?;
        let comment = map_comment(am.insert(&txn).await.map_err(map_err)?);
        let sql = format!(
            "UPDATE community_posts SET comments_count = comments_count + 1 WHERE post_id = {}",
            post_id
        );
        txn.execute_unprepared(&sql).await.map_err(map_err)?;
        txn.commit().await.map_err(map_err)?;
        Ok(comment)
    }

    async fn update_comment(
        &self,
        comment_id: u64,
        content: Option<String>,
        status: Option<ArticleStatus>,
    ) -> Result<Comment, AppError> {
        let existing = community_comments::Entity::find_by_id(comment_id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or(AppError::NotFound("comment not found".into()))?;
        let mut am: community_comments::ActiveModel = existing.into();
        if let Some(c) = content {
            am.content = Set(c);
        }
        if let Some(s) = status {
            am.status = Set(s.to_i8());
        }
        am.updated_at = Set(chrono::Utc::now().naive_utc());
        Ok(map_comment(am.update(&self.db).await.map_err(map_err)?))
    }

    async fn delete_comment(&self, comment_id: u64) -> Result<bool, AppError> {
        let txn = self.db.begin().await.map_err(map_err)?;
        let existing = community_comments::Entity::find_by_id(comment_id)
            .one(&txn)
            .await
            .map_err(map_err)?;
        let post_id = match existing {
            Some(ref m) => m.post_id,
            None => return Ok(false),
        };
        let ok = community_comments::Entity::delete_by_id(comment_id)
            .exec(&txn)
            .await
            .map_err(map_err)?
            .rows_affected
            > 0;
        if ok {
            let sql = format!(
                "UPDATE community_posts SET comments_count = GREATEST(comments_count - 1, 0) \
                 WHERE post_id = {}",
                post_id
            );
            txn.execute_unprepared(&sql).await.map_err(map_err)?;
        }
        txn.commit().await.map_err(map_err)?;
        Ok(ok)
    }

    // -- Media ---------------------------------------------------------------

    async fn list_media_by_post(&self, post_id: u64) -> Result<Vec<PostMedia>, AppError> {
        community_post_media::Entity::find()
            .filter(community_post_media::Column::PostId.eq(post_id))
            .all(&self.db)
            .await
            .map_err(map_err)
            .map(|v| v.into_iter().map(map_media).collect())
    }

    async fn save_media(&self, new_media: NewPostMedia) -> Result<PostMedia, AppError> {
        let now = chrono::Utc::now();
        let am = community_post_media::ActiveModel {
            post_id: Set(new_media.post_id),
            media_type: Set(new_media.media_type),
            mime_type: Set(new_media.mime_type),
            media_data: Set(new_media.media_data),
            created_at: Set(now.naive_utc()),
            ..Default::default()
        };
        Ok(map_media(am.insert(&self.db).await.map_err(map_err)?))
    }

    // -- Likes ----------------------------------------------------------------

    async fn like_post(&self, post_id: u64, user_id: u64) -> Result<(), AppError> {
        let txn = self.db.begin().await.map_err(map_err)?;
        let existing = content_likes::Entity::find()
            .filter(content_likes::Column::UserId.eq(user_id))
            .filter(content_likes::Column::ContentType.eq("community_post"))
            .filter(content_likes::Column::ContentId.eq(post_id))
            .one(&txn)
            .await
            .map_err(map_err)?;
        if existing.is_some() {
            txn.rollback().await.map_err(map_err)?;
            return Ok(()); // already liked – idempotent
        }
        let now = chrono::Utc::now().naive_utc();
        let am = content_likes::ActiveModel {
            user_id: Set(user_id),
            content_type: Set("community_post".to_owned()),
            content_id: Set(post_id),
            created_at: Set(now),
            ..Default::default()
        };
        am.insert(&txn).await.map_err(map_err)?;

        // bump the denormalized counter
        let sql = format!(
            "UPDATE community_posts SET likes_count = likes_count + 1 WHERE post_id = {}",
            post_id
        );
        txn.execute_unprepared(&sql).await.map_err(map_err)?;
        txn.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn unlike_post(&self, post_id: u64, user_id: u64) -> Result<(), AppError> {
        let txn = self.db.begin().await.map_err(map_err)?;
        let deleted = content_likes::Entity::delete_many()
            .filter(content_likes::Column::UserId.eq(user_id))
            .filter(content_likes::Column::ContentType.eq("community_post"))
            .filter(content_likes::Column::ContentId.eq(post_id))
            .exec(&txn)
            .await
            .map_err(map_err)?
            .rows_affected;
        if deleted > 0 {
            let sql = format!(
                "UPDATE community_posts SET likes_count = GREATEST(likes_count - 1, 0) WHERE post_id = {}",
                post_id
            );
            txn.execute_unprepared(&sql).await.map_err(map_err)?;
        }
        txn.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn like_comment(&self, comment_id: u64, user_id: u64) -> Result<(), AppError> {
        let txn = self.db.begin().await.map_err(map_err)?;
        let existing = content_likes::Entity::find()
            .filter(content_likes::Column::UserId.eq(user_id))
            .filter(content_likes::Column::ContentType.eq("community_comment"))
            .filter(content_likes::Column::ContentId.eq(comment_id))
            .one(&txn)
            .await
            .map_err(map_err)?;
        if existing.is_some() {
            txn.rollback().await.map_err(map_err)?;
            return Ok(());
        }
        let now = chrono::Utc::now().naive_utc();
        let am = content_likes::ActiveModel {
            user_id: Set(user_id),
            content_type: Set("community_comment".to_owned()),
            content_id: Set(comment_id),
            created_at: Set(now),
            ..Default::default()
        };
        am.insert(&txn).await.map_err(map_err)?;

        let sql = format!(
            "UPDATE community_comments SET likes_count = likes_count + 1 WHERE comment_id = {}",
            comment_id
        );
        txn.execute_unprepared(&sql).await.map_err(map_err)?;
        txn.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn unlike_comment(&self, comment_id: u64, user_id: u64) -> Result<(), AppError> {
        let txn = self.db.begin().await.map_err(map_err)?;
        let deleted = content_likes::Entity::delete_many()
            .filter(content_likes::Column::UserId.eq(user_id))
            .filter(content_likes::Column::ContentType.eq("community_comment"))
            .filter(content_likes::Column::ContentId.eq(comment_id))
            .exec(&txn)
            .await
            .map_err(map_err)?
            .rows_affected;
        if deleted > 0 {
            let sql = format!(
                "UPDATE community_comments SET likes_count = GREATEST(likes_count - 1, 0) WHERE comment_id = {}",
                comment_id
            );
            txn.execute_unprepared(&sql).await.map_err(map_err)?;
        }
        txn.commit().await.map_err(map_err)?;
        Ok(())
    }
}
