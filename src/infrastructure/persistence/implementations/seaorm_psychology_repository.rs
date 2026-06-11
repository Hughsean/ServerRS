use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};

use crate::domain::psychology::{
    KnowledgeFavorite, NewContentLike, NewKnowledgeFavorite, NewPsychologyArticle,
    NewPsychologyCategory, NewPsychologyQna, NewPsychologyResource, PsychologyArticle,
    PsychologyCategory, PsychologyQna, PsychologyRepository, PsychologyResource,
};
use crate::shared::error::AppError;

use super::super::entities::{
    content_likes, psychology_articles, psychology_categories, psychology_qna,
    psychology_resources, user_knowledge_favorites,
};

pub struct SeaOrmPsychologyRepository {
    db: DatabaseConnection,
}

impl SeaOrmPsychologyRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

fn tags_to_string(v: Option<sea_orm::prelude::Json>) -> Option<String> {
    v.map(|j| serde_json::to_string(&j).unwrap_or_default())
}

fn string_to_json(s: Option<String>) -> Option<sea_orm::prelude::Json> {
    s.and_then(|s| serde_json::from_str(&s).ok())
}

fn map_category(m: psychology_categories::Model) -> PsychologyCategory {
    PsychologyCategory {
        id: m.category_id as u64,
        parent_id: m.parent_id.map(|id| id as u64),
        name: m.category_name,
        description: m.description,
        sort_order: m.sort_order,
        is_enabled: m.status != 0,
        created_at: m.created_at,
    }
}

fn map_article(m: psychology_articles::Model) -> PsychologyArticle {
    PsychologyArticle {
        id: m.article_id,
        category_id: Some(m.category_id as u64),
        title: m.title,
        summary: m.summary,
        content: m.content,
        tags: tags_to_string(m.tags),
        view_count: m.view_count as i64,
        like_count: m.like_count as i64,
        is_published: m.is_published != 0,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

fn map_qna(m: psychology_qna::Model) -> PsychologyQna {
    PsychologyQna {
        id: m.qna_id,
        category_id: Some(m.category_id as u64),
        question: m.question,
        answer: m.answer,
        tags: tags_to_string(m.tags),
        view_count: m.view_count as i64,
        like_count: m.like_count as i64,
        is_published: m.status != 0,
        created_at: m.created_at,
    }
}

fn map_resource(m: psychology_resources::Model) -> PsychologyResource {
    PsychologyResource {
        id: m.resource_id,
        category_id: Some(m.category_id as u64),
        title: m.title,
        description: m.description,
        resource_type: m.resource_type,
        object_id: None,
        external_url: m.external_url,
        tags: tags_to_string(m.tags),
        view_count: m.view_count as i64,
        like_count: m.like_count as i64,
        is_published: m.status != 0,
        created_at: m.created_at,
    }
}

fn map_favorite(m: user_knowledge_favorites::Model) -> KnowledgeFavorite {
    KnowledgeFavorite {
        id: m.favorite_id,
        user_id: m.user_id,
        content_type: m.content_type,
        content_id: m.content_id,
        created_at: m.created_at,
    }
}

fn normalize_psychology_content_type(raw: &str) -> Result<&'static str, AppError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "article" | "psychology_article" => Ok("article"),
        "qna" | "psychology_qna" => Ok("qna"),
        "resource" | "psychology_resource" => Ok("resource"),
        _ => Err(AppError::Validation(
            "content_type must be article, qna, or resource".into(),
        )),
    }
}

async fn ensure_content_exists<C>(
    db: &C,
    content_type: &str,
    content_id: u64,
) -> Result<(), AppError>
where
    C: ConnectionTrait,
{
    let exists = match content_type {
        "article" => psychology_articles::Entity::find_by_id(content_id)
            .one(db)
            .await
            .map_err(map_err)?
            .is_some(),
        "qna" => psychology_qna::Entity::find_by_id(content_id)
            .one(db)
            .await
            .map_err(map_err)?
            .is_some(),
        "resource" => psychology_resources::Entity::find_by_id(content_id)
            .one(db)
            .await
            .map_err(map_err)?
            .is_some(),
        _ => false,
    };

    if exists {
        Ok(())
    } else {
        Err(AppError::NotFound(format!(
            "{content_type} {content_id} not found"
        )))
    }
}

async fn update_like_count<C>(
    db: &C,
    content_type: &str,
    content_id: u64,
    liked: bool,
) -> Result<(), AppError>
where
    C: ConnectionTrait,
{
    let assignment = if liked {
        "like_count = like_count + 1"
    } else {
        "like_count = GREATEST(like_count - 1, 0)"
    };

    let sql = match content_type {
        "article" => {
            format!("UPDATE psychology_articles SET {assignment} WHERE article_id = {content_id}")
        }
        "qna" => format!("UPDATE psychology_qna SET {assignment} WHERE qna_id = {content_id}"),
        "resource" => {
            format!("UPDATE psychology_resources SET {assignment} WHERE resource_id = {content_id}")
        }
        _ => {
            return Err(AppError::Validation(
                "content_type must be article, qna, or resource".into(),
            ));
        }
    };

    db.execute_unprepared(&sql).await.map_err(map_err)?;
    Ok(())
}

#[async_trait]
impl PsychologyRepository for SeaOrmPsychologyRepository {
    // ── Categories ──

    async fn list_categories(&self) -> Result<Vec<PsychologyCategory>, AppError> {
        psychology_categories::Entity::find()
            .order_by_asc(psychology_categories::Column::SortOrder)
            .all(&self.db)
            .await
            .map_err(map_err)
            .map(|v| v.into_iter().map(map_category).collect())
    }

    async fn find_category_by_id(&self, id: u64) -> Result<Option<PsychologyCategory>, AppError> {
        psychology_categories::Entity::find_by_id(id as u16)
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|opt| opt.map(map_category))
    }

    async fn create_category(
        &self,
        new: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError> {
        let now = chrono::Utc::now();
        let am = psychology_categories::ActiveModel {
            category_name: Set(new.name),
            parent_id: Set(new.parent_id.map(|id| id as u16)),
            description: Set(new.description),
            sort_order: Set(new.sort_order),
            status: Set(if new.is_enabled { 1_i8 } else { 0_i8 }),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        };
        am.insert(&self.db).await.map_err(map_err).map(map_category)
    }

    async fn update_category(
        &self,
        id: u64,
        new: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError> {
        let existing = psychology_categories::Entity::find_by_id(id as u16)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::NotFound("category not found".into()))?;
        let mut am: psychology_categories::ActiveModel = existing.into();
        am.category_name = Set(new.name);
        am.parent_id = Set(new.parent_id.map(|id| id as u16));
        am.description = Set(new.description);
        am.sort_order = Set(new.sort_order);
        am.status = Set(if new.is_enabled { 1_i8 } else { 0_i8 });
        am.updated_at = Set(chrono::Utc::now());
        am.update(&self.db).await.map_err(map_err).map(map_category)
    }

    async fn delete_category(&self, id: u64) -> Result<bool, AppError> {
        psychology_categories::Entity::delete_by_id(id as u16)
            .exec(&self.db)
            .await
            .map_err(map_err)
            .map(|r| r.rows_affected > 0)
    }

    // ── Articles ──

    async fn list_articles(
        &self,
        page: u64,
        page_size: u64,
        search: Option<String>,
        category_id: Option<u64>,
        is_featured: Option<bool>,
    ) -> Result<(Vec<PsychologyArticle>, u64), AppError> {
        let mut base = psychology_articles::Entity::find()
            .filter(psychology_articles::Column::IsPublished.eq(1_i8));

        if let Some(cid) = category_id {
            base = base.filter(psychology_articles::Column::CategoryId.eq(cid as u16));
        }

        if let Some(featured) = is_featured {
            base = base.filter(psychology_articles::Column::IsFeatured.eq(if featured {
                1_i8
            } else {
                0_i8
            }));
        }

        if let Some(ref q) = search {
            if !q.is_empty() {
                base = base.filter(
                    sea_orm::Condition::any()
                        .add(psychology_articles::Column::Title.contains(q))
                        .add(psychology_articles::Column::Summary.contains(q)),
                );
            }
        }

        let paginator = base
            .order_by_desc(psychology_articles::Column::CreatedAt)
            .paginate(&self.db, page_size);

        let total = paginator.num_items().await.map_err(map_err)?;
        let models = paginator
            .fetch_page(page.max(1) - 1)
            .await
            .map_err(map_err)?;

        Ok((models.into_iter().map(map_article).collect(), total))
    }

    async fn find_article_by_id(&self, id: u64) -> Result<Option<PsychologyArticle>, AppError> {
        let opt = psychology_articles::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?;
        if let Some(_m) = opt {
            // Increment view_count via raw SQL because content is ignore-marked,
            // so the ActiveModel cannot round-trip through update safely.
            let sql = format!(
                "UPDATE psychology_articles SET view_count = view_count + 1 WHERE article_id = {}",
                id
            );
            self.db.execute_unprepared(&sql).await.map_err(map_err)?;
            // Re-fetch to get the updated row (including content from LONGTEXT)
            let updated = psychology_articles::Entity::find_by_id(id)
                .one(&self.db)
                .await
                .map_err(map_err)?
                .ok_or_else(|| AppError::NotFound("article not found".into()))?;
            Ok(Some(map_article(updated)))
        } else {
            Ok(None)
        }
    }

    async fn create_article(
        &self,
        new: NewPsychologyArticle,
    ) -> Result<PsychologyArticle, AppError> {
        let now = chrono::Utc::now();
        let am = psychology_articles::ActiveModel {
            category_id: Set(new.category_id.unwrap_or(0) as u16),
            title: Set(new.title.clone()),
            summary: Set(new.summary.clone()),
            author: Set(None),
            source: Set(None),
            tags: Set(string_to_json(new.tags.clone())),
            cover_image: Set(None),
            view_count: Set(0_u32),
            like_count: Set(0_u32),
            is_featured: Set(0_i8),
            is_published: Set(if new.is_published { 1_i8 } else { 0_i8 }),
            publish_date: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        };
        let result = am.insert(&self.db).await.map_err(map_err)?;
        let inserted_id = result.article_id;

        // content is ignore-marked (LONGTEXT alias); set via raw SQL
        let escaped_content = new.content.replace('\'', "''");
        let sql = format!(
            "UPDATE psychology_articles SET content = '{}' WHERE article_id = {}",
            escaped_content, inserted_id
        );
        self.db.execute_unprepared(&sql).await.map_err(map_err)?;

        // Re-fetch to return the complete model
        let final_article = psychology_articles::Entity::find_by_id(inserted_id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::Internal("created article not found".into()))?;
        Ok(map_article(final_article))
    }

    async fn update_article(
        &self,
        id: u64,
        new: NewPsychologyArticle,
    ) -> Result<PsychologyArticle, AppError> {
        // Use raw SQL update for content (ignore-marked) + standard fields
        // We build the SQL manually to set content alongside other fields.
        let now = chrono::Utc::now()
            .format("%Y-%m-%d %H:%M:%S%.f")
            .to_string();
        let escaped_title = new.title.replace('\'', "''");
        let escaped_summary = new.summary.as_ref().map(|s| s.replace('\'', "''"));
        let escaped_content = new.content.replace('\'', "''");
        let summary_sql = match &escaped_summary {
            Some(s) => format!("'{}'", s),
            None => "NULL".to_string(),
        };
        let tags_sql = match &new.tags {
            Some(t) => format!("'{}'", t.replace('\'', "''")),
            None => "NULL".to_string(),
        };
        let is_pub = if new.is_published { 1_i8 } else { 0_i8 };
        let cat_id = new.category_id.unwrap_or(0);

        let sql = format!(
            "UPDATE psychology_articles SET \
             category_id = {}, title = '{}', summary = {}, content = '{}', \
             tags = {}, is_published = {}, updated_at = '{}' \
             WHERE article_id = {}",
            cat_id, escaped_title, summary_sql, escaped_content, tags_sql, is_pub, now, id
        );
        self.db.execute_unprepared(&sql).await.map_err(map_err)?;

        // Re-fetch to return complete model
        let updated = psychology_articles::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::NotFound("article not found".into()))?;
        Ok(map_article(updated))
    }

    async fn delete_article(&self, id: u64) -> Result<bool, AppError> {
        psychology_articles::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .map_err(map_err)
            .map(|r| r.rows_affected > 0)
    }

    // ── QnA ──

    async fn list_qnas(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        is_verified: Option<bool>,
    ) -> Result<(Vec<PsychologyQna>, u64), AppError> {
        let mut base =
            psychology_qna::Entity::find().filter(psychology_qna::Column::Status.eq(1_i8));

        if let Some(cid) = category_id {
            base = base.filter(psychology_qna::Column::CategoryId.eq(cid as u16));
        }

        if let Some(verified) = is_verified {
            base = base.filter(psychology_qna::Column::IsVerified.eq(if verified {
                1_i8
            } else {
                0_i8
            }));
        }

        let paginator = base
            .order_by_desc(psychology_qna::Column::CreatedAt)
            .paginate(&self.db, page_size);

        let total = paginator.num_items().await.map_err(map_err)?;
        let models = paginator
            .fetch_page(page.max(1) - 1)
            .await
            .map_err(map_err)?;

        Ok((models.into_iter().map(map_qna).collect(), total))
    }

    async fn find_qna_by_id(&self, id: u64) -> Result<Option<PsychologyQna>, AppError> {
        let opt = psychology_qna::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?;
        if let Some(_m) = opt {
            // Increment view_count via raw SQL (answer is ignore-marked)
            let sql = format!(
                "UPDATE psychology_qna SET view_count = view_count + 1 WHERE qna_id = {}",
                id
            );
            self.db.execute_unprepared(&sql).await.map_err(map_err)?;
            let updated = psychology_qna::Entity::find_by_id(id)
                .one(&self.db)
                .await
                .map_err(map_err)?
                .ok_or_else(|| AppError::NotFound("qna not found".into()))?;
            Ok(Some(map_qna(updated)))
        } else {
            Ok(None)
        }
    }

    async fn create_qna(&self, new: NewPsychologyQna) -> Result<PsychologyQna, AppError> {
        let now = chrono::Utc::now();
        let am = psychology_qna::ActiveModel {
            category_id: Set(new.category_id.unwrap_or(0) as u16),
            question: Set(new.question.clone()),
            tags: Set(string_to_json(new.tags.clone())),
            view_count: Set(0_u32),
            like_count: Set(0_u32),
            is_verified: Set(0_i8),
            status: Set(if new.is_published { 1_i8 } else { 0_i8 }),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        };
        let result = am.insert(&self.db).await.map_err(map_err)?;
        let inserted_id = result.qna_id;

        // answer is ignore-marked (LONGTEXT alias); set via raw SQL
        let escaped_answer = new.answer.replace('\'', "''");
        let sql = format!(
            "UPDATE psychology_qna SET answer = '{}' WHERE qna_id = {}",
            escaped_answer, inserted_id
        );
        self.db.execute_unprepared(&sql).await.map_err(map_err)?;

        let final_qna = psychology_qna::Entity::find_by_id(inserted_id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::Internal("created qna not found".into()))?;
        Ok(map_qna(final_qna))
    }

    async fn update_qna(&self, id: u64, new: NewPsychologyQna) -> Result<PsychologyQna, AppError> {
        let now = chrono::Utc::now()
            .format("%Y-%m-%d %H:%M:%S%.f")
            .to_string();
        let escaped_question = new.question.replace('\'', "''");
        let escaped_answer = new.answer.replace('\'', "''");
        let tags_sql = match &new.tags {
            Some(t) => format!("'{}'", t.replace('\'', "''")),
            None => "NULL".to_string(),
        };
        let is_pub = if new.is_published { 1_i8 } else { 0_i8 };
        let cat_id = new.category_id.unwrap_or(0);

        let sql = format!(
            "UPDATE psychology_qna SET \
             category_id = {}, question = '{}', answer = '{}', \
             tags = {}, status = {}, updated_at = '{}' \
             WHERE qna_id = {}",
            cat_id, escaped_question, escaped_answer, tags_sql, is_pub, now, id
        );
        self.db.execute_unprepared(&sql).await.map_err(map_err)?;

        let updated = psychology_qna::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::NotFound("qna not found".into()))?;
        Ok(map_qna(updated))
    }

    async fn delete_qna(&self, id: u64) -> Result<bool, AppError> {
        psychology_qna::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .map_err(map_err)
            .map(|r| r.rows_affected > 0)
    }

    // ── Resources ──

    async fn list_resources(
        &self,
        page: u64,
        page_size: u64,
        category_id: Option<u64>,
        resource_type: Option<String>,
    ) -> Result<(Vec<PsychologyResource>, u64), AppError> {
        let mut base = psychology_resources::Entity::find()
            .filter(psychology_resources::Column::Status.eq(1_i8));

        if let Some(cid) = category_id {
            base = base.filter(psychology_resources::Column::CategoryId.eq(cid as u16));
        }

        if let Some(ref rt) = resource_type {
            if !rt.is_empty() {
                base = base.filter(psychology_resources::Column::ResourceType.eq(rt));
            }
        }

        let paginator = base
            .order_by_desc(psychology_resources::Column::CreatedAt)
            .paginate(&self.db, page_size);

        let total = paginator.num_items().await.map_err(map_err)?;
        let models = paginator
            .fetch_page(page.max(1) - 1)
            .await
            .map_err(map_err)?;

        Ok((models.into_iter().map(map_resource).collect(), total))
    }

    async fn find_resource_by_id(&self, id: u64) -> Result<Option<PsychologyResource>, AppError> {
        let opt = psychology_resources::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?;
        if let Some(_m) = opt {
            // Increment view_count via raw SQL (file_data is ignore-marked BLOB alias)
            let sql = format!(
                "UPDATE psychology_resources SET view_count = view_count + 1 WHERE resource_id = {}",
                id
            );
            self.db.execute_unprepared(&sql).await.map_err(map_err)?;
            let updated = psychology_resources::Entity::find_by_id(id)
                .one(&self.db)
                .await
                .map_err(map_err)?
                .ok_or_else(|| AppError::NotFound("resource not found".into()))?;
            Ok(Some(map_resource(updated)))
        } else {
            Ok(None)
        }
    }

    async fn create_resource(
        &self,
        new: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError> {
        let now = chrono::Utc::now();
        let am = psychology_resources::ActiveModel {
            category_id: Set(new.category_id.unwrap_or(0) as u16),
            resource_type: Set(new.resource_type),
            title: Set(new.title),
            description: Set(new.description),
            external_url: Set(new.external_url),
            file_size: Set(None),
            mime_type: Set(None),
            duration: Set(None),
            tags: Set(string_to_json(new.tags)),
            view_count: Set(0_u32),
            like_count: Set(0_u32),
            status: Set(if new.is_published { 1_i8 } else { 0_i8 }),
            created_at: Set(now),
            updated_at: Set(now),
            thumbnail: Set(None),
            ..Default::default()
        };
        am.insert(&self.db).await.map_err(map_err).map(map_resource)
    }

    async fn update_resource(
        &self,
        id: u64,
        new: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError> {
        let existing = psychology_resources::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::NotFound("resource not found".into()))?;
        let mut am: psychology_resources::ActiveModel = existing.into();
        am.category_id = Set(new.category_id.unwrap_or(0) as u16);
        am.resource_type = Set(new.resource_type);
        am.title = Set(new.title);
        am.description = Set(new.description);
        am.external_url = Set(new.external_url);
        am.tags = Set(string_to_json(new.tags));
        am.status = Set(if new.is_published { 1_i8 } else { 0_i8 });
        am.updated_at = Set(chrono::Utc::now());
        am.update(&self.db).await.map_err(map_err).map(map_resource)
    }

    async fn delete_resource(&self, id: u64) -> Result<bool, AppError> {
        psychology_resources::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .map_err(map_err)
            .map(|r| r.rows_affected > 0)
    }

    // ── Favorites ──

    async fn toggle_favorite(&self, new: NewKnowledgeFavorite) -> Result<bool, AppError> {
        let existing = user_knowledge_favorites::Entity::find()
            .filter(user_knowledge_favorites::Column::UserId.eq(new.user_id))
            .filter(user_knowledge_favorites::Column::ContentType.eq(new.content_type.clone()))
            .filter(user_knowledge_favorites::Column::ContentId.eq(new.content_id))
            .one(&self.db)
            .await
            .map_err(map_err)?;

        if let Some(m) = existing {
            user_knowledge_favorites::Entity::delete_by_id(m.favorite_id)
                .exec(&self.db)
                .await
                .map_err(map_err)?;
            Ok(false)
        } else {
            // Use raw INSERT to avoid any ActiveModel field discrepancies
            let sql = format!(
                "INSERT INTO user_knowledge_favorites (user_id, content_type, content_id, created_at) \
                 VALUES ({}, '{}', {}, NOW())",
                new.user_id,
                new.content_type.replace('\'', "''"),
                new.content_id
            );
            self.db.execute_unprepared(&sql).await.map_err(map_err)?;
            Ok(true)
        }
    }

    async fn check_favorite(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError> {
        user_knowledge_favorites::Entity::find()
            .filter(user_knowledge_favorites::Column::UserId.eq(user_id))
            .filter(user_knowledge_favorites::Column::ContentType.eq(content_type))
            .filter(user_knowledge_favorites::Column::ContentId.eq(content_id))
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|opt| opt.is_some())
    }

    async fn list_favorites(
        &self,
        user_id: u64,
        content_type: Option<&str>,
    ) -> Result<Vec<KnowledgeFavorite>, AppError> {
        let mut q = user_knowledge_favorites::Entity::find()
            .filter(user_knowledge_favorites::Column::UserId.eq(user_id))
            .order_by_desc(user_knowledge_favorites::Column::CreatedAt);
        if let Some(ct) = content_type {
            q = q.filter(user_knowledge_favorites::Column::ContentType.eq(ct));
        }
        q.all(&self.db)
            .await
            .map_err(map_err)
            .map(|v| v.into_iter().map(map_favorite).collect())
    }

    // ── Likes ──

    async fn toggle_like(&self, new: NewContentLike) -> Result<bool, AppError> {
        let content_type = normalize_psychology_content_type(&new.content_type)?;
        ensure_content_exists(&self.db, content_type, new.content_id).await?;

        let txn = self.db.begin().await.map_err(map_err)?;
        let existing = content_likes::Entity::find()
            .filter(content_likes::Column::UserId.eq(new.user_id))
            .filter(content_likes::Column::ContentType.eq(content_type))
            .filter(content_likes::Column::ContentId.eq(new.content_id))
            .one(&txn)
            .await
            .map_err(map_err)?;

        let liked = if let Some(record) = existing {
            content_likes::Entity::delete_by_id(record.like_id)
                .exec(&txn)
                .await
                .map_err(map_err)?;
            false
        } else {
            let am = content_likes::ActiveModel {
                user_id: Set(new.user_id),
                content_type: Set(content_type.to_string()),
                content_id: Set(new.content_id),
                created_at: Set(chrono::Utc::now().naive_utc()),
                ..Default::default()
            };
            am.insert(&txn).await.map_err(map_err)?;
            true
        };

        update_like_count(&txn, content_type, new.content_id, liked).await?;
        txn.commit().await.map_err(map_err)?;
        Ok(liked)
    }
}
