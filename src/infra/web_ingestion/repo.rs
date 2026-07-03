//! 所有网页知识摄取表的 SeaORM 仓库实现。

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, Set, Statement,
    TransactionTrait, Unchanged, Value,
};
use serde_json::Value as JsonValue;

use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repo::*;
use crate::domain::web_ingestion::status::publish_status;
use crate::infra::repo::entities::*;

fn map_db_err(e: sea_orm::DbErr) -> WebIngestionError {
    WebIngestionError::Internal(e.to_string())
}

fn to_utc(dt: NaiveDateTime) -> DateTime<Utc> {
    dt.and_utc()
}
fn to_naive(dt: DateTime<Utc>) -> NaiveDateTime {
    dt.naive_utc()
}

// ============================================================================
// WebSourceRepository
// ============================================================================

pub struct WebSourceRepo {
    db: DatabaseConnection,
}

impl WebSourceRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl WebSourceRepoT for WebSourceRepo {
    async fn find_by_id(&self, id: u64) -> Result<Option<WebSource>, WebIngestionError> {
        let row = web_sources::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_web_source))
    }
    async fn list_enabled(&self) -> Result<Vec<WebSource>, WebIngestionError> {
        let rows = web_sources::Entity::find()
            .filter(web_sources::Column::Enabled.eq(1))
            .filter(web_sources::Column::DeletedAt.is_null())
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(rows.into_iter().map(model_to_web_source).collect())
    }
    async fn insert(&self, src: NewWebSource) -> Result<WebSource, WebIngestionError> {
        let active = web_sources::ActiveModel {
            name: Set(src.name),
            description: Set(src.description),
            approval_status: Set(src.approval_status),
            trust_level: Set(src.trust_level),
            auto_publish: Set(if src.auto_publish { 1 } else { 0 }),
            allowed_domains: Set(src.allowed_domains),
            default_language: Set(src.default_language),
            enabled: Set(if src.enabled { 1 } else { 0 }),
            ..Default::default()
        };
        let model = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_web_source(model))
    }
    async fn update(&self, id: u64, src: NewWebSource) -> Result<WebSource, WebIngestionError> {
        let existing = web_sources::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "web_source".into(),
                id,
            })?;
        let mut active: web_sources::ActiveModel = existing.into();
        active.name = Set(src.name);
        active.description = Set(src.description);
        active.approval_status = Set(src.approval_status);
        active.trust_level = Set(src.trust_level);
        active.auto_publish = Set(if src.auto_publish { 1 } else { 0 });
        active.allowed_domains = Set(src.allowed_domains);
        active.default_language = Set(src.default_language);
        active.enabled = Set(if src.enabled { 1 } else { 0 });
        let model = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_web_source(model))
    }
}

fn model_to_web_source(m: web_sources::Model) -> WebSource {
    WebSource {
        id: m.id,
        name: m.name,
        description: m.description,
        approval_status: m.approval_status,
        trust_level: m.trust_level,
        auto_publish: m.auto_publish != 0,
        allowed_domains: m.allowed_domains,
        default_language: m.default_language,
        enabled: m.enabled != 0,
        created_at: to_utc(m.created_at),
        updated_at: to_utc(m.updated_at),
        deleted_at: m.deleted_at.map(to_utc),
    }
}

// ============================================================================
// WebSourceUrlRepository
// ============================================================================

pub struct WebSourceUrlRepo {
    db: DatabaseConnection,
}

impl WebSourceUrlRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl WebSourceUrlRepoT for WebSourceUrlRepo {
    async fn find_by_id(&self, id: u64) -> Result<Option<WebSourceUrl>, WebIngestionError> {
        let row = web_source_urls::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_ws_url))
    }
    async fn find_by_source_and_hash(
        &self,
        source_id: u64,
        url_hash: &str,
    ) -> Result<Option<WebSourceUrl>, WebIngestionError> {
        let row = web_source_urls::Entity::find()
            .filter(web_source_urls::Column::SourceId.eq(source_id))
            .filter(web_source_urls::Column::UrlHash.eq(url_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_ws_url))
    }
    async fn list_by_source(&self, source_id: u64) -> Result<Vec<WebSourceUrl>, WebIngestionError> {
        let rows = web_source_urls::Entity::find()
            .filter(web_source_urls::Column::SourceId.eq(source_id))
            .filter(web_source_urls::Column::DeletedAt.is_null())
            .order_by_asc(web_source_urls::Column::LastCrawledAt)
            .order_by_asc(web_source_urls::Column::Id)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(rows.into_iter().map(model_to_ws_url).collect())
    }
    async fn list_due_for_crawl(
        &self,
        _now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<WebSourceUrl>, WebIngestionError> {
        // Simple approach: list enabled URLs ordered by last_crawled_at (NULLs first),
        // application layer checks if enough time has passed
        let rows = web_source_urls::Entity::find()
            .filter(web_source_urls::Column::Enabled.eq(1))
            .filter(web_source_urls::Column::DeletedAt.is_null())
            .order_by_asc(web_source_urls::Column::LastCrawledAt)
            .limit(Some(limit))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(rows.into_iter().map(model_to_ws_url).collect())
    }
    async fn upsert(&self, url: NewWebSourceUrl) -> Result<WebSourceUrl, WebIngestionError> {
        let existing = self
            .find_by_source_and_hash(url.source_id, &url.url_hash)
            .await?;
        if let Some(row) = existing {
            let mut active: web_source_urls::ActiveModel = row_to_ws_url_active(row);
            active.url = Set(url.url);
            active.canonical_url = Set(url.canonical_url);
            active.crawl_interval_secs = Set(url.crawl_interval_secs);
            let model = active.update(&self.db).await.map_err(map_db_err)?;
            Ok(model_to_ws_url(model))
        } else {
            let active = web_source_urls::ActiveModel {
                source_id: Set(url.source_id),
                url: Set(url.url),
                canonical_url: Set(url.canonical_url),
                url_hash: Set(url.url_hash),
                crawl_interval_secs: Set(url.crawl_interval_secs),
                ..Default::default()
            };
            let model = active.insert(&self.db).await.map_err(map_db_err)?;
            Ok(model_to_ws_url(model))
        }
    }
    async fn mark_crawled(
        &self,
        id: u64,
        content_hash: &str,
        crawled_at: DateTime<Utc>,
    ) -> Result<(), WebIngestionError> {
        let row = web_source_urls::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "web_source_url".into(),
                id,
            })?;
        let mut active: web_source_urls::ActiveModel = row.into();
        active.last_crawled_at = Set(Some(to_naive(crawled_at)));
        active.last_content_hash = Set(Some(content_hash.to_string()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}

fn model_to_ws_url(m: web_source_urls::Model) -> WebSourceUrl {
    WebSourceUrl {
        id: m.id,
        source_id: m.source_id,
        url: m.url,
        canonical_url: m.canonical_url,
        url_hash: m.url_hash,
        enabled: m.enabled != 0,
        crawl_interval_secs: m.crawl_interval_secs,
        last_crawled_at: m.last_crawled_at.map(to_utc),
        last_content_hash: m.last_content_hash,
        created_at: to_utc(m.created_at),
        updated_at: to_utc(m.updated_at),
        deleted_at: m.deleted_at.map(to_utc),
    }
}

fn row_to_ws_url_active(row: WebSourceUrl) -> web_source_urls::ActiveModel {
    web_source_urls::ActiveModel {
        id: Unchanged(row.id),
        source_id: Unchanged(row.source_id),
        url: Unchanged(row.url),
        canonical_url: Unchanged(row.canonical_url),
        url_hash: Unchanged(row.url_hash),
        enabled: Unchanged(if row.enabled { 1 } else { 0 }),
        crawl_interval_secs: Unchanged(row.crawl_interval_secs),
        last_crawled_at: Unchanged(row.last_crawled_at.map(to_naive)),
        last_content_hash: Unchanged(row.last_content_hash),
        created_at: Unchanged(to_naive(row.created_at)),
        updated_at: Unchanged(to_naive(row.updated_at)),
        deleted_at: Unchanged(row.deleted_at.map(to_naive)),
    }
}

// ============================================================================
// WebCrawlJobRepository
// ============================================================================

pub struct WebCrawlJobRepo {
    db: DatabaseConnection,
}

impl WebCrawlJobRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl WebCrawlJobRepoT for WebCrawlJobRepo {
    async fn find_by_id(&self, id: u64) -> Result<Option<WebCrawlJob>, WebIngestionError> {
        let row = web_crawl_jobs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_crawl_job))
    }
    async fn insert(&self, job: NewWebCrawlJob) -> Result<WebCrawlJob, WebIngestionError> {
        let active = web_crawl_jobs::ActiveModel {
            source_id: Set(job.source_id),
            status: Set(job.status),
            scheduled_at: Set(to_naive(job.scheduled_at)),
            ..Default::default()
        };
        let model = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_crawl_job(model))
    }
    async fn update_status(
        &self,
        id: u64,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<(), WebIngestionError> {
        let row = web_crawl_jobs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "web_crawl_job".into(),
                id,
            })?;
        let mut active: web_crawl_jobs::ActiveModel = row.into();
        active.status = Set(status.to_string());
        active.last_error = Set(last_error.map(|s| s.to_string()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
    async fn mark_started(&self, id: u64) -> Result<(), WebIngestionError> {
        let row = web_crawl_jobs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "web_crawl_job".into(),
                id,
            })?;
        let mut active: web_crawl_jobs::ActiveModel = row.into();
        active.status = Set("running".into());
        active.started_at = Set(Some(Utc::now().naive_utc()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
    async fn mark_finished(&self, id: u64, status: &str) -> Result<(), WebIngestionError> {
        let row = web_crawl_jobs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "web_crawl_job".into(),
                id,
            })?;
        let mut active: web_crawl_jobs::ActiveModel = row.into();
        active.status = Set(status.to_string());
        active.finished_at = Set(Some(Utc::now().naive_utc()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}

fn model_to_crawl_job(m: web_crawl_jobs::Model) -> WebCrawlJob {
    WebCrawlJob {
        id: m.id,
        source_id: m.source_id,
        status: m.status,
        scheduled_at: to_utc(m.scheduled_at),
        started_at: m.started_at.map(to_utc),
        finished_at: m.finished_at.map(to_utc),
        last_error: m.last_error,
        created_at: to_utc(m.created_at),
        updated_at: to_utc(m.updated_at),
    }
}

// ============================================================================
// WebPageRepository
// ============================================================================

pub struct WebPageRepo {
    db: DatabaseConnection,
}

impl WebPageRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl WebPageRepoT for WebPageRepo {
    async fn find_by_id(&self, id: u64) -> Result<Option<WebPage>, WebIngestionError> {
        let row = web_pages::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_web_page))
    }
    async fn find_by_source_and_hash(
        &self,
        source_id: u64,
        url_hash: &str,
    ) -> Result<Option<WebPage>, WebIngestionError> {
        let row = web_pages::Entity::find()
            .filter(web_pages::Column::SourceId.eq(source_id))
            .filter(web_pages::Column::UrlHash.eq(url_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_web_page))
    }
    async fn upsert(&self, page: NewWebPage) -> Result<WebPage, WebIngestionError> {
        let existing = self
            .find_by_source_and_hash(page.source_id, &page.url_hash)
            .await?;
        if let Some(row) = existing {
            let mut active: web_pages::ActiveModel = row_to_web_page_active(row);
            active.url = Set(page.url);
            active.canonical_url = Set(page.canonical_url);
            active.source_url_id = Set(page.source_url_id);
            let model = active.update(&self.db).await.map_err(map_db_err)?;
            Ok(model_to_web_page(model))
        } else {
            let active = web_pages::ActiveModel {
                source_id: Set(page.source_id),
                source_url_id: Set(page.source_url_id),
                url: Set(page.url),
                canonical_url: Set(page.canonical_url),
                url_hash: Set(page.url_hash),
                ..Default::default()
            };
            let model = active.insert(&self.db).await.map_err(map_db_err)?;
            Ok(model_to_web_page(model))
        }
    }
    async fn mark_fetched(
        &self,
        id: u64,
        content_hash: &str,
        run_id: u64,
        fetched_at: DateTime<Utc>,
    ) -> Result<(), WebIngestionError> {
        let row = web_pages::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "web_page".into(),
                id,
            })?;
        let mut active: web_pages::ActiveModel = row.into();
        active.latest_content_hash = Set(Some(content_hash.to_string()));
        active.latest_success_run_id = Set(Some(run_id));
        active.last_fetched_at = Set(Some(to_naive(fetched_at)));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}

fn model_to_web_page(m: web_pages::Model) -> WebPage {
    WebPage {
        id: m.id,
        source_id: m.source_id,
        source_url_id: m.source_url_id,
        url: m.url,
        canonical_url: m.canonical_url,
        url_hash: m.url_hash,
        latest_content_hash: m.latest_content_hash,
        latest_success_run_id: m.latest_success_run_id,
        last_fetched_at: m.last_fetched_at.map(to_utc),
        created_at: to_utc(m.created_at),
        updated_at: to_utc(m.updated_at),
        deleted_at: m.deleted_at.map(to_utc),
    }
}

fn row_to_web_page_active(row: WebPage) -> web_pages::ActiveModel {
    web_pages::ActiveModel {
        id: Unchanged(row.id),
        source_id: Unchanged(row.source_id),
        source_url_id: Unchanged(row.source_url_id),
        url: Unchanged(row.url),
        canonical_url: Unchanged(row.canonical_url),
        url_hash: Unchanged(row.url_hash),
        latest_content_hash: Unchanged(row.latest_content_hash),
        latest_success_run_id: Unchanged(row.latest_success_run_id),
        last_fetched_at: Unchanged(row.last_fetched_at.map(to_naive)),
        created_at: Unchanged(to_naive(row.created_at)),
        updated_at: Unchanged(to_naive(row.updated_at)),
        deleted_at: Unchanged(row.deleted_at.map(to_naive)),
    }
}

// ============================================================================
// IngestionRunRepository
// ============================================================================

pub struct IngestionRunRepo {
    db: DatabaseConnection,
}

impl IngestionRunRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl IngestionRunRepoT for IngestionRunRepo {
    async fn find_by_id(
        &self,
        id: u64,
    ) -> Result<Option<KnowledgeIngestionRun>, WebIngestionError> {
        let row = knowledge_ingestion_runs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_run))
    }
    async fn find_by_run_key(
        &self,
        run_key: &str,
    ) -> Result<Option<KnowledgeIngestionRun>, WebIngestionError> {
        let row = knowledge_ingestion_runs::Entity::find()
            .filter(knowledge_ingestion_runs::Column::RunKey.eq(run_key))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_run))
    }
    async fn find_by_content_key(
        &self,
        content_key: &str,
    ) -> Result<Option<KnowledgeIngestionRun>, WebIngestionError> {
        let row = knowledge_ingestion_runs::Entity::find()
            .filter(knowledge_ingestion_runs::Column::ContentKey.eq(content_key))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_run))
    }
    async fn insert(
        &self,
        run: NewIngestionRun,
    ) -> Result<KnowledgeIngestionRun, WebIngestionError> {
        let active = knowledge_ingestion_runs::ActiveModel {
            source_id: Set(run.source_id),
            source_url_id: Set(run.source_url_id),
            crawl_job_id: Set(run.crawl_job_id),
            page_id: Set(run.page_id),
            content_hash: Set(run.content_hash),
            content_key: Set(run.content_key),
            run_key: Set(run.run_key),
            version_key: Set(run.version_key),
            status: Set("pending".into()),
            stage: Set("pending".into()),
            ..Default::default()
        };
        let model = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_run(model))
    }
    async fn update_status_stage(
        &self,
        id: u64,
        expected_status: &str,
        expected_stage: &str,
        new_status: &str,
        new_stage: &str,
        last_error: Option<&str>,
    ) -> Result<bool, WebIngestionError> {
        use crate::domain::web_ingestion::state_machine::can_transition_run;

        // Validate the declared edge before touching the row. The actual CAS is
        // the UPDATE below; rows_affected=0 means another worker moved it first.
        if !can_transition_run(expected_status, expected_stage, new_status, new_stage) {
            return Err(WebIngestionError::InvalidTransition {
                from_status: expected_status.to_string(),
                from_stage: expected_stage.to_string(),
                to_status: new_status.to_string(),
                to_stage: new_stage.to_string(),
            });
        }

        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE knowledge_ingestion_runs \
             SET status = ?, stage = ?, last_error = ?, updated_at = NOW() \
             WHERE id = ? AND status = ? AND stage = ?",
            vec![
                Value::String(Some(new_status.to_string().into())),
                Value::String(Some(new_stage.to_string().into())),
                Value::String(last_error.map(|s| s.to_string().into())),
                Value::BigUnsigned(Some(id)),
                Value::String(Some(expected_status.to_string().into())),
                Value::String(Some(expected_stage.to_string().into())),
            ],
        );
        let result = self.db.execute_raw(stmt).await.map_err(map_db_err)?;
        if result.rows_affected() > 0 {
            return Ok(true);
        }

        let exists = knowledge_ingestion_runs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .is_some();
        if exists {
            Ok(false)
        } else {
            Err(WebIngestionError::NotFound {
                entity: "knowledge_ingestion_run".into(),
                id,
            })
        }
    }
    async fn update_distill_result(
        &self,
        id: u64,
        llm_provider: &str,
        llm_model: &str,
        llm_prompt_version: &str,
        llm_input_tokens: Option<u32>,
        llm_output_tokens: Option<u32>,
        quality_score: f64,
        quality_result: JsonValue,
        risk_flags: JsonValue,
        should_publish: bool,
    ) -> Result<(), WebIngestionError> {
        let row = knowledge_ingestion_runs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_ingestion_run".into(),
                id,
            })?;
        let mut active: knowledge_ingestion_runs::ActiveModel = row.into();
        active.llm_provider = Set(Some(llm_provider.to_string()));
        active.llm_model = Set(Some(llm_model.to_string()));
        active.llm_prompt_version = Set(Some(llm_prompt_version.to_string()));
        active.llm_input_tokens = Set(llm_input_tokens);
        active.llm_output_tokens = Set(llm_output_tokens);
        active.quality_score = Set(Some(quality_score));
        active.quality_result = Set(Some(quality_result));
        active.risk_flags = Set(Some(risk_flags));
        active.should_publish = Set(Some(if should_publish { 1 } else { 0 }));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
    async fn update_embedding_info(
        &self,
        id: u64,
        embedding_provider: &str,
        embedding_model: &str,
        embedding_dimension: u32,
    ) -> Result<(), WebIngestionError> {
        let row = knowledge_ingestion_runs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_ingestion_run".into(),
                id,
            })?;
        let mut active: knowledge_ingestion_runs::ActiveModel = row.into();
        active.embedding_provider = Set(Some(embedding_provider.to_string()));
        active.embedding_model = Set(Some(embedding_model.to_string()));
        active.embedding_dimension = Set(Some(embedding_dimension));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
    async fn mark_started(&self, id: u64) -> Result<(), WebIngestionError> {
        let row = knowledge_ingestion_runs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_ingestion_run".into(),
                id,
            })?;
        let mut active: knowledge_ingestion_runs::ActiveModel = row.into();
        active.started_at = Set(Some(Utc::now().naive_utc()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
    async fn mark_finished(&self, id: u64) -> Result<(), WebIngestionError> {
        let row = knowledge_ingestion_runs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_ingestion_run".into(),
                id,
            })?;
        let mut active: knowledge_ingestion_runs::ActiveModel = row.into();
        active.finished_at = Set(Some(Utc::now().naive_utc()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
    async fn update_artifacts(
        &self,
        id: u64,
        fetched_body_text: Option<&str>,
        clean_text: Option<&str>,
        distilled_json: Option<JsonValue>,
    ) -> Result<(), WebIngestionError> {
        let row = knowledge_ingestion_runs::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_ingestion_run".into(),
                id,
            })?;
        let mut active: knowledge_ingestion_runs::ActiveModel = row.into();
        if let Some(v) = fetched_body_text {
            active.fetched_body_text = Set(Some(v.to_string()));
        }
        if let Some(v) = clean_text {
            active.clean_text = Set(Some(v.to_string()));
        }
        if let Some(v) = distilled_json {
            active.distilled_json = Set(Some(v));
        }
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
    async fn find_latest_for_page(
        &self,
        page_id: u64,
    ) -> Result<Option<KnowledgeIngestionRun>, WebIngestionError> {
        let row = knowledge_ingestion_runs::Entity::find()
            .filter(knowledge_ingestion_runs::Column::PageId.eq(page_id))
            .order_by_desc(knowledge_ingestion_runs::Column::CreatedAt)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_run))
    }
}

fn model_to_run(m: knowledge_ingestion_runs::Model) -> KnowledgeIngestionRun {
    KnowledgeIngestionRun {
        id: m.id,
        source_id: m.source_id,
        source_url_id: m.source_url_id,
        crawl_job_id: m.crawl_job_id,
        page_id: m.page_id,
        content_hash: m.content_hash,
        content_key: m.content_key,
        run_key: m.run_key,
        version_key: m.version_key,
        status: m.status,
        stage: m.stage,
        llm_provider: m.llm_provider,
        llm_model: m.llm_model,
        llm_prompt_version: m.llm_prompt_version,
        llm_input_tokens: m.llm_input_tokens,
        llm_output_tokens: m.llm_output_tokens,
        chunker_version: m.chunker_version,
        embedding_provider: m.embedding_provider,
        embedding_model: m.embedding_model,
        embedding_dimension: m.embedding_dimension,
        quality_score: m.quality_score,
        quality_result: m.quality_result,
        risk_flags: m.risk_flags,
        should_publish: m.should_publish.map(|v| v != 0),
        last_error: m.last_error,
        retry_count: m.retry_count,
        started_at: m.started_at.map(to_utc),
        finished_at: m.finished_at.map(to_utc),
        created_at: to_utc(m.created_at),
        updated_at: to_utc(m.updated_at),
        fetched_body_text: m.fetched_body_text,
        clean_text: m.clean_text,
        distilled_json: m.distilled_json,
    }
}

// ============================================================================
// PublishRecordRepository
// ============================================================================

pub struct PublishRecordRepo {
    db: DatabaseConnection,
}

impl PublishRecordRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl PublishRecordRepoT for PublishRecordRepo {
    async fn find_by_id(
        &self,
        id: u64,
    ) -> Result<Option<KnowledgePublishRecord>, WebIngestionError> {
        let row = knowledge_publish_records::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_pr))
    }
    async fn find_active_by_page(
        &self,
        source_id: u64,
        page_id: u64,
    ) -> Result<Option<KnowledgePublishRecord>, WebIngestionError> {
        let active_key = format!("{source_id}:{page_id}");
        let row = knowledge_publish_records::Entity::find()
            .filter(knowledge_publish_records::Column::ActivePageKey.eq(active_key))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_pr))
    }
    async fn find_by_run_id(
        &self,
        run_id: u64,
    ) -> Result<Option<KnowledgePublishRecord>, WebIngestionError> {
        let row = knowledge_publish_records::Entity::find()
            .filter(knowledge_publish_records::Column::RunId.eq(run_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_pr))
    }
    async fn insert(
        &self,
        record: NewPublishRecord,
    ) -> Result<KnowledgePublishRecord, WebIngestionError> {
        let active = knowledge_publish_records::ActiveModel {
            source_id: Set(record.source_id),
            page_id: Set(record.page_id),
            run_id: Set(record.run_id),
            document_id: Set(record.document_id),
            version_key: Set(record.version_key),
            content_hash: Set(record.content_hash),
            active_page_key: Set(record.active_page_key),
            publish_status: Set("staged".into()),
            active: Set(0),
            ..Default::default()
        };
        let model = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_pr(model))
    }
    async fn set_active(
        &self,
        id: u64,
        active: bool,
        _active_page_key: Option<&str>,
        publish_status: &str,
    ) -> Result<(), WebIngestionError> {
        let row = knowledge_publish_records::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_publish_record".into(),
                id,
            })?;
        let active_page_key = if active {
            Some(format!("{}:{}", row.source_id, row.page_id))
        } else {
            None
        };
        let mut am: knowledge_publish_records::ActiveModel = row.into();
        am.active = Set(if active { 1 } else { 0 });
        am.active_page_key = Set(active_page_key);
        am.publish_status = Set(publish_status.to_string());
        if active {
            am.activated_at = Set(Some(Utc::now().naive_utc()));
        } else {
            am.superseded_at = Set(Some(Utc::now().naive_utc()));
        }
        am.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
    async fn find_active_sibling(
        &self,
        record_id: u64,
    ) -> Result<Option<KnowledgePublishRecord>, WebIngestionError> {
        let record = self.find_by_id(record_id).await?;
        let Some(record) = record else {
            return Ok(None);
        };
        self.find_active_by_page(record.source_id, record.page_id)
            .await
    }
    async fn lock_page_for_publish(
        &self,
        _source_id: u64,
        page_id: u64,
    ) -> Result<(), WebIngestionError> {
        // Lock the web_pages row with FOR UPDATE using parameterized query.
        // This serializes publish/rollback per page.
        //
        // IMPORTANT: For the lock to be meaningful, this call MUST be wrapped
        // in a SeaORM transaction. The caller (publish_service) uses individual
        // repository calls which each run in autocommit — so the lock is
        // released immediately after this call returns.
        //
        // TODO: Refactor publish/rollback to use a proper transactional session
        // (e.g. pass a &DatabaseConnection with an open transaction through
        //  the repository methods, or use a Unit of Work pattern).
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT id FROM web_pages WHERE id = ? FOR UPDATE",
            vec![Value::BigUnsigned(Some(page_id))],
        );
        self.db.execute_raw(stmt).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn publish_in_tx(
        &self,
        publish_record_id: u64,
    ) -> Result<PublishOutcome, WebIngestionError> {
        let txn = self.db.begin().await.map_err(map_db_err)?;

        // 1. Load target record.
        let target = knowledge_publish_records::Entity::find_by_id(publish_record_id)
            .one(&txn)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_publish_record".into(),
                id: publish_record_id,
            })?;

        // 2. Page lock — meaningful because it is inside this transaction.
        lock_page_in(&txn, target.page_id).await?;

        // Idempotent: already the active record.
        if target.active != 0 && target.publish_status == publish_status::PUBLISHED {
            txn.commit().await.map_err(map_db_err)?;
            return Ok(PublishOutcome {
                activated_record_id: target.id,
                activated_document_id: target.document_id,
                deactivated_record_id: None,
                deactivated_document_id: None,
                was_already_active: true,
            });
        }

        if target.publish_status != publish_status::STAGED {
            txn.rollback().await.ok();
            return Err(WebIngestionError::Internal(format!(
                "cannot publish record {} in '{}' status",
                target.id, target.publish_status
            )));
        }

        // 3. Supersede the current active record for this page (if any/different).
        let active_key = format!("{}:{}", target.source_id, target.page_id);
        let current = knowledge_publish_records::Entity::find()
            .filter(knowledge_publish_records::Column::ActivePageKey.eq(active_key))
            .one(&txn)
            .await
            .map_err(map_db_err)?;

        let (deactivated_record_id, deactivated_document_id) = match current {
            Some(ref cur) if cur.id != target.id => {
                deactivate_record_in(&txn, cur, publish_status::SUPERSEDED).await?;
                (Some(cur.id), Some(cur.document_id))
            }
            _ => (None, None),
        };

        // 4. Activate target.
        activate_record_in(&txn, &target).await?;

        txn.commit().await.map_err(map_db_err)?;
        Ok(PublishOutcome {
            activated_record_id: target.id,
            activated_document_id: target.document_id,
            deactivated_record_id,
            deactivated_document_id,
            was_already_active: false,
        })
    }

    async fn rollback_in_tx(
        &self,
        current_record_id: u64,
        target_record_id: u64,
    ) -> Result<PublishOutcome, WebIngestionError> {
        let txn = self.db.begin().await.map_err(map_db_err)?;

        let current = knowledge_publish_records::Entity::find_by_id(current_record_id)
            .one(&txn)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_publish_record (current)".into(),
                id: current_record_id,
            })?;
        let target = knowledge_publish_records::Entity::find_by_id(target_record_id)
            .one(&txn)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_publish_record (target)".into(),
                id: target_record_id,
            })?;

        if current.source_id != target.source_id || current.page_id != target.page_id {
            txn.rollback().await.ok();
            return Err(WebIngestionError::Internal(
                "rollback: current and target must belong to the same page".into(),
            ));
        }
        if current.active == 0 {
            txn.rollback().await.ok();
            return Err(WebIngestionError::Internal(
                "rollback: current record is not active".into(),
            ));
        }
        // Cannot roll back TO a rejected/failed/dead version.
        if matches!(
            target.publish_status.as_str(),
            publish_status::FAILED | "rejected" | "dead"
        ) {
            txn.rollback().await.ok();
            return Err(WebIngestionError::Internal(format!(
                "rollback: target record {} is in '{}' status — not a rollback candidate",
                target.id, target.publish_status
            )));
        }

        lock_page_in(&txn, current.page_id).await?;

        deactivate_record_in(&txn, &current, publish_status::ROLLED_BACK).await?;
        activate_record_in(&txn, &target).await?;

        txn.commit().await.map_err(map_db_err)?;
        Ok(PublishOutcome {
            activated_record_id: target.id,
            activated_document_id: target.document_id,
            deactivated_record_id: Some(current.id),
            deactivated_document_id: Some(current.document_id),
            was_already_active: false,
        })
    }
}

/// FOR UPDATE lock on a web_pages row, inside a transaction.
async fn lock_page_in<C: ConnectionTrait>(txn: &C, page_id: u64) -> Result<(), WebIngestionError> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id FROM web_pages WHERE id = ? FOR UPDATE",
        vec![Value::BigUnsigned(Some(page_id))],
    );
    txn.execute_raw(stmt).await.map_err(map_db_err)?;
    Ok(())
}

/// Deactivate a publish record + its document + its manifests, inside a tx.
async fn deactivate_record_in<C: ConnectionTrait>(
    txn: &C,
    record: &knowledge_publish_records::Model,
    new_status: &str,
) -> Result<(), WebIngestionError> {
    let mut am: knowledge_publish_records::ActiveModel = record.clone().into();
    am.active = Set(0);
    am.publish_status = Set(new_status.to_string());
    am.superseded_at = Set(Some(Utc::now().naive_utc()));
    am.active_page_key = Set(None);
    am.update(txn).await.map_err(map_db_err)?;

    set_document_status_in(txn, record.document_id, 0).await?;
    set_manifests_active_in(txn, record.id, false).await?;
    Ok(())
}

/// Activate a publish record + its document + its manifests, inside a tx.
async fn activate_record_in<C: ConnectionTrait>(
    txn: &C,
    record: &knowledge_publish_records::Model,
) -> Result<(), WebIngestionError> {
    let mut am: knowledge_publish_records::ActiveModel = record.clone().into();
    am.active = Set(1);
    am.publish_status = Set(publish_status::PUBLISHED.to_string());
    am.activated_at = Set(Some(Utc::now().naive_utc()));
    am.active_page_key = Set(Some(format!("{}:{}", record.source_id, record.page_id)));
    am.update(txn).await.map_err(map_db_err)?;

    set_document_status_in(txn, record.document_id, 1).await?;
    set_manifests_active_in(txn, record.id, true).await?;
    Ok(())
}

/// Flip knowledge_documents.status (1=active/visible, 0=staged/hidden), in a tx.
async fn set_document_status_in<C: ConnectionTrait>(
    txn: &C,
    document_id: u64,
    status: i8,
) -> Result<(), WebIngestionError> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE knowledge_documents SET status = ?, updated_at = NOW() WHERE document_id = ?",
        vec![
            Value::TinyInt(Some(status)),
            Value::BigUnsigned(Some(document_id)),
        ],
    );
    txn.execute_raw(stmt).await.map_err(map_db_err)?;
    Ok(())
}

/// Flip chunk + vector manifest active flags for a publish record, in a tx.
async fn set_manifests_active_in<C: ConnectionTrait>(
    txn: &C,
    publish_record_id: u64,
    active: bool,
) -> Result<(), WebIngestionError> {
    let a = if active { 1i8 } else { 0i8 };
    let chunk_stmt = Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE knowledge_chunk_manifests SET active = ?, updated_at = NOW() WHERE publish_record_id = ?",
        vec![
            Value::TinyInt(Some(a)),
            Value::BigUnsigned(Some(publish_record_id)),
        ],
    );
    txn.execute_raw(chunk_stmt).await.map_err(map_db_err)?;
    let vec_stmt = Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE knowledge_vector_manifests SET active = ?, updated_at = NOW() WHERE publish_record_id = ?",
        vec![
            Value::TinyInt(Some(a)),
            Value::BigUnsigned(Some(publish_record_id)),
        ],
    );
    txn.execute_raw(vec_stmt).await.map_err(map_db_err)?;
    Ok(())
}

fn model_to_pr(m: knowledge_publish_records::Model) -> KnowledgePublishRecord {
    KnowledgePublishRecord {
        id: m.id,
        source_id: m.source_id,
        page_id: m.page_id,
        run_id: m.run_id,
        document_id: m.document_id,
        version_key: m.version_key,
        content_hash: m.content_hash,
        publish_status: m.publish_status,
        active: m.active != 0,
        active_page_key: m.active_page_key,
        activated_at: m.activated_at.map(to_utc),
        superseded_at: m.superseded_at.map(to_utc),
        superseded_by_record_id: m.superseded_by_record_id,
        rolled_back_from_record_id: m.rolled_back_from_record_id,
        created_at: to_utc(m.created_at),
        updated_at: to_utc(m.updated_at),
    }
}

// ============================================================================
// ChunkManifestRepository
// ============================================================================

pub struct ChunkManifestRepo {
    db: DatabaseConnection,
}

impl ChunkManifestRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ChunkManifestRepoT for ChunkManifestRepo {
    async fn find_by_version_and_hash(
        &self,
        version_key: &str,
        chunk_hash: &str,
    ) -> Result<Option<KnowledgeChunkManifest>, WebIngestionError> {
        let row = knowledge_chunk_manifests::Entity::find()
            .filter(knowledge_chunk_manifests::Column::VersionKey.eq(version_key))
            .filter(knowledge_chunk_manifests::Column::ChunkHash.eq(chunk_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_cm))
    }
    async fn find_by_chunk_id(
        &self,
        chunk_id: u64,
    ) -> Result<Option<KnowledgeChunkManifest>, WebIngestionError> {
        let row = knowledge_chunk_manifests::Entity::find()
            .filter(knowledge_chunk_manifests::Column::ChunkId.eq(chunk_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_cm))
    }
    async fn insert_batch(
        &self,
        manifests: &[NewChunkManifest],
    ) -> Result<Vec<KnowledgeChunkManifest>, WebIngestionError> {
        let mut results = Vec::with_capacity(manifests.len());
        for m in manifests {
            let active = knowledge_chunk_manifests::ActiveModel {
                publish_record_id: Set(m.publish_record_id),
                run_id: Set(m.run_id),
                document_id: Set(m.document_id),
                chunk_id: Set(m.chunk_id),
                version_key: Set(m.version_key.clone()),
                chunk_hash: Set(m.chunk_hash.clone()),
                chunk_type: Set(m.chunk_type.clone()),
                chunk_index: Set(m.chunk_index),
                active: Set(0),
                ..Default::default()
            };
            let model = active.insert(&self.db).await.map_err(map_db_err)?;
            results.push(model_to_cm(model));
        }
        Ok(results)
    }
    async fn set_active_by_publish_record(
        &self,
        publish_record_id: u64,
        active: bool,
    ) -> Result<(), WebIngestionError> {
        let rows = knowledge_chunk_manifests::Entity::find()
            .filter(knowledge_chunk_manifests::Column::PublishRecordId.eq(publish_record_id))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        let target = if active { 1 } else { 0 };
        for row in rows {
            let mut am = row.into_active_model();
            am.active = Set(target);
            am.update(&self.db).await.map_err(map_db_err)?;
        }
        Ok(())
    }
    async fn list_by_publish_record(
        &self,
        publish_record_id: u64,
    ) -> Result<Vec<KnowledgeChunkManifest>, WebIngestionError> {
        let rows = knowledge_chunk_manifests::Entity::find()
            .filter(knowledge_chunk_manifests::Column::PublishRecordId.eq(publish_record_id))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(rows.into_iter().map(model_to_cm).collect())
    }
}

fn model_to_cm(m: knowledge_chunk_manifests::Model) -> KnowledgeChunkManifest {
    KnowledgeChunkManifest {
        id: m.id,
        publish_record_id: m.publish_record_id,
        run_id: m.run_id,
        document_id: m.document_id,
        chunk_id: m.chunk_id,
        version_key: m.version_key,
        chunk_hash: m.chunk_hash,
        chunk_type: m.chunk_type,
        chunk_index: m.chunk_index,
        active: m.active != 0,
        created_at: to_utc(m.created_at),
        updated_at: to_utc(m.updated_at),
    }
}

// ============================================================================
// VectorManifestRepository
// ============================================================================

pub struct VectorManifestRepo {
    db: DatabaseConnection,
}

impl VectorManifestRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl VectorManifestRepoT for VectorManifestRepo {
    async fn find_by_collection_and_point(
        &self,
        collection: &str,
        point_id: &str,
    ) -> Result<Option<KnowledgeVectorManifest>, WebIngestionError> {
        let row = knowledge_vector_manifests::Entity::find()
            .filter(knowledge_vector_manifests::Column::QdrantCollection.eq(collection))
            .filter(knowledge_vector_manifests::Column::QdrantPointId.eq(point_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_vm))
    }
    async fn find_by_chunk_and_model(
        &self,
        chunk_id: u64,
        embedding_model: &str,
    ) -> Result<Option<KnowledgeVectorManifest>, WebIngestionError> {
        let row = knowledge_vector_manifests::Entity::find()
            .filter(knowledge_vector_manifests::Column::ChunkId.eq(chunk_id))
            .filter(knowledge_vector_manifests::Column::EmbeddingModel.eq(embedding_model))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(model_to_vm))
    }
    async fn insert_batch(
        &self,
        manifests: &[NewVectorManifest],
    ) -> Result<Vec<KnowledgeVectorManifest>, WebIngestionError> {
        let mut results = Vec::with_capacity(manifests.len());
        for m in manifests {
            let active = knowledge_vector_manifests::ActiveModel {
                publish_record_id: Set(m.publish_record_id),
                run_id: Set(m.run_id),
                document_id: Set(m.document_id),
                chunk_id: Set(m.chunk_id),
                chunk_hash: Set(m.chunk_hash.clone()),
                qdrant_collection: Set(m.qdrant_collection.clone()),
                qdrant_point_id: Set(m.qdrant_point_id.clone()),
                embedding_provider: Set(m.embedding_provider.clone()),
                embedding_model: Set(m.embedding_model.clone()),
                embedding_dimension: Set(m.embedding_dimension),
                active: Set(0),
                ..Default::default()
            };
            let model = active.insert(&self.db).await.map_err(map_db_err)?;
            results.push(model_to_vm(model));
        }
        Ok(results)
    }
    async fn set_active_by_publish_record(
        &self,
        publish_record_id: u64,
        active: bool,
    ) -> Result<(), WebIngestionError> {
        let rows = knowledge_vector_manifests::Entity::find()
            .filter(knowledge_vector_manifests::Column::PublishRecordId.eq(publish_record_id))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        let target = if active { 1 } else { 0 };
        for row in rows {
            let mut am = row.into_active_model();
            am.active = Set(target);
            am.update(&self.db).await.map_err(map_db_err)?;
        }
        Ok(())
    }
    async fn list_by_publish_record(
        &self,
        publish_record_id: u64,
    ) -> Result<Vec<KnowledgeVectorManifest>, WebIngestionError> {
        let rows = knowledge_vector_manifests::Entity::find()
            .filter(knowledge_vector_manifests::Column::PublishRecordId.eq(publish_record_id))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(rows.into_iter().map(model_to_vm).collect())
    }
}

fn model_to_vm(m: knowledge_vector_manifests::Model) -> KnowledgeVectorManifest {
    KnowledgeVectorManifest {
        id: m.id,
        publish_record_id: m.publish_record_id,
        run_id: m.run_id,
        document_id: m.document_id,
        chunk_id: m.chunk_id,
        chunk_hash: m.chunk_hash,
        qdrant_collection: m.qdrant_collection,
        qdrant_point_id: m.qdrant_point_id,
        embedding_provider: m.embedding_provider,
        embedding_model: m.embedding_model,
        embedding_dimension: m.embedding_dimension,
        active: m.active != 0,
        created_at: to_utc(m.created_at),
        updated_at: to_utc(m.updated_at),
    }
}

// ============================================================================
// OutboxRepository
// ============================================================================

pub struct OutboxRepo {
    db: DatabaseConnection,
}

impl OutboxRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl OutboxRepoT for OutboxRepo {
    async fn insert_event(&self, event: NewOutboxEvent) -> Result<DomainEvent, WebIngestionError> {
        // Idempotent: INSERT … ON DUPLICATE KEY UPDATE (no-op) using parameterized query.
        let payload_str = serde_json::to_string(&event.payload).unwrap_or_default();
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO domain_event_outbox (event_key, event_type, aggregate_type, \
             aggregate_id, payload, max_retries, status) \
             VALUES (?, ?, ?, ?, ?, ?, 'pending') \
             ON DUPLICATE KEY UPDATE updated_at = updated_at",
            vec![
                Value::String(Some(event.event_key.clone().into())),
                Value::String(Some(event.event_type.into())),
                Value::String(Some(event.aggregate_type.into())),
                Value::BigUnsigned(Some(event.aggregate_id)),
                Value::String(Some(payload_str.into())),
                Value::Unsigned(Some(event.max_retries)),
            ],
        );
        self.db.execute_raw(stmt).await.map_err(map_db_err)?;

        // Fetch the (possibly pre-existing) row
        let row = domain_event_outbox::Entity::find()
            .filter(domain_event_outbox::Column::EventKey.eq(&event.event_key))
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::Internal("outbox insert failed".into()))?;
        Ok(model_to_event(row))
    }
    async fn claim_batch(
        &self,
        claim_token: &str,
        lock_ttl_secs: u32,
        limit: u64,
    ) -> Result<Vec<DomainEvent>, WebIngestionError> {
        let quotas = vec![OutboxClaimQuota {
            event_types: Vec::new(),
            exclude_event_types: Vec::new(),
            limit,
        }];
        self.claim_batch_by_quotas(claim_token, lock_ttl_secs, &quotas, limit)
            .await
    }
    async fn claim_batch_by_quotas(
        &self,
        claim_token: &str,
        lock_ttl_secs: u32,
        quotas: &[OutboxClaimQuota],
        max_total: u64,
    ) -> Result<Vec<DomainEvent>, WebIngestionError> {
        if max_total == 0 || quotas.is_empty() {
            return Ok(Vec::new());
        }

        let priority_sql = outbox_event_priority_sql();
        let txn = self.db.begin().await.map_err(map_db_err)?;
        let mut ids = Vec::new();

        for quota in quotas {
            let remaining = max_total.saturating_sub(ids.len() as u64);
            if remaining == 0 {
                break;
            }
            let quota_limit = quota.limit.min(remaining);
            if quota_limit == 0 {
                continue;
            }

            let mut event_scope_sql = String::new();
            let mut values = Vec::new();
            append_event_type_scope(&mut event_scope_sql, &mut values, quota);
            values.push(Value::BigUnsigned(Some(quota_limit)));

            let select_sql = format!(
                "SELECT id FROM domain_event_outbox \
                 WHERE (status IN ('pending','failed') \
                        OR (status = 'processing' AND locked_until < NOW())) \
                   AND (next_retry_at IS NULL OR next_retry_at <= NOW()) \
                   {event_scope_sql} \
                 ORDER BY {priority_sql}, created_at ASC \
                 LIMIT ? \
                 FOR UPDATE SKIP LOCKED"
            );
            let select_stmt =
                Statement::from_sql_and_values(DatabaseBackend::MySql, select_sql, values);
            let selected = txn.query_all_raw(select_stmt).await.map_err(map_db_err)?;
            for row in selected {
                let id: u64 = row
                    .try_get("", "id")
                    .map_err(|e| WebIngestionError::Internal(e.to_string()))?;
                ids.push(id);
            }
        }

        if ids.is_empty() {
            txn.commit().await.map_err(map_db_err)?;
            return Ok(Vec::new());
        }

        let placeholders = std::iter::repeat("?")
            .take(ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let update_sql = format!(
            "UPDATE domain_event_outbox \
             SET status = 'processing', locked_by = ?, \
                 locked_until = DATE_ADD(NOW(), INTERVAL ? SECOND), updated_at = NOW() \
             WHERE id IN ({placeholders})"
        );
        let mut values = vec![
            Value::String(Some(claim_token.into())),
            Value::Unsigned(Some(lock_ttl_secs)),
        ];
        values.extend(ids.iter().map(|id| Value::BigUnsigned(Some(*id))));
        let update_stmt =
            Statement::from_sql_and_values(DatabaseBackend::MySql, update_sql, values);
        txn.execute_raw(update_stmt).await.map_err(map_db_err)?;

        let mut rows = domain_event_outbox::Entity::find()
            .filter(domain_event_outbox::Column::Id.is_in(ids.clone()))
            .filter(domain_event_outbox::Column::LockedBy.eq(claim_token))
            .filter(domain_event_outbox::Column::Status.eq("processing"))
            .all(&txn)
            .await
            .map_err(map_db_err)?;
        rows.sort_by_key(|row| {
            ids.iter()
                .position(|id| *id == row.id)
                .unwrap_or(usize::MAX)
        });
        txn.commit().await.map_err(map_db_err)?;
        Ok(rows.into_iter().map(model_to_event).collect())
    }
    async fn claim_one_by_quota(
        &self,
        claim_token: &str,
        lock_ttl_secs: u32,
        quota: &OutboxClaimQuota,
    ) -> Result<Option<DomainEvent>, WebIngestionError> {
        let priority_sql = outbox_event_priority_sql();
        let txn = self.db.begin().await.map_err(map_db_err)?;

        // Reuse append_event_type_scope for event-type filtering
        let mut event_scope_sql = String::new();
        let mut values = Vec::new();
        append_event_type_scope(&mut event_scope_sql, &mut values, quota);

        let select_sql = format!(
            "SELECT id FROM domain_event_outbox \
             WHERE (status IN ('pending','failed') \
                    OR (status = 'processing' AND locked_until < NOW())) \
               AND (next_retry_at IS NULL OR next_retry_at <= NOW()) \
               {event_scope_sql} \
             ORDER BY {priority_sql}, created_at ASC \
             LIMIT 1 \
             FOR UPDATE SKIP LOCKED"
        );
        let select_stmt =
            Statement::from_sql_and_values(DatabaseBackend::MySql, select_sql, values);
        let selected = txn.query_one_raw(select_stmt).await.map_err(map_db_err)?;

        let Some(row) = selected else {
            txn.commit().await.map_err(map_db_err)?;
            return Ok(None);
        };
        let id: u64 = row
            .try_get("", "id")
            .map_err(|e| WebIngestionError::Internal(e.to_string()))?;

        let update_sql = format!(
            "UPDATE domain_event_outbox \
             SET status = 'processing', locked_by = ?, \
                 locked_until = DATE_ADD(NOW(), INTERVAL ? SECOND), updated_at = NOW() \
             WHERE id = ?"
        );
        let update_stmt = Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            update_sql,
            vec![
                Value::String(Some(claim_token.into())),
                Value::Unsigned(Some(lock_ttl_secs)),
                Value::BigUnsigned(Some(id)),
            ],
        );
        txn.execute_raw(update_stmt).await.map_err(map_db_err)?;

        let row = domain_event_outbox::Entity::find()
            .filter(domain_event_outbox::Column::Id.eq(id))
            .filter(domain_event_outbox::Column::LockedBy.eq(claim_token))
            .filter(domain_event_outbox::Column::Status.eq("processing"))
            .one(&txn)
            .await
            .map_err(map_db_err)?;

        txn.commit().await.map_err(map_db_err)?;
        Ok(row.map(model_to_event))
    }
    async fn renew_lock(
        &self,
        id: u64,
        claim_token: &str,
        lock_ttl_secs: u32,
    ) -> Result<bool, WebIngestionError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE domain_event_outbox \
             SET locked_until = DATE_ADD(NOW(), INTERVAL ? SECOND), updated_at = NOW() \
             WHERE id = ? AND status = 'processing' AND locked_by = ?",
            vec![
                Value::Unsigned(Some(lock_ttl_secs)),
                Value::BigUnsigned(Some(id)),
                Value::String(Some(claim_token.into())),
            ],
        );
        let result = self.db.execute_raw(stmt).await.map_err(map_db_err)?;
        Ok(result.rows_affected() > 0)
    }
    async fn mark_published(&self, id: u64, claim_token: &str) -> Result<bool, WebIngestionError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE domain_event_outbox SET status = 'published', published_at = NOW(), updated_at = NOW() \
             WHERE id = ? AND status = 'processing' AND locked_by = ?",
            vec![
                Value::BigUnsigned(Some(id)),
                Value::String(Some(claim_token.into())),
            ],
        );
        let result = self.db.execute_raw(stmt).await.map_err(map_db_err)?;
        Ok(result.rows_affected() > 0)
    }
    async fn mark_failed_or_dead(
        &self,
        id: u64,
        claim_token: &str,
        last_error: &str,
        next_retry_at: DateTime<Utc>,
        is_dead: bool,
    ) -> Result<bool, WebIngestionError> {
        let new_status = if is_dead { "dead" } else { "failed" };
        let next_retry = next_retry_at.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE domain_event_outbox SET status = ?, retry_count = retry_count + 1, \
             next_retry_at = ?, last_error = ?, locked_by = NULL, \
             locked_until = NULL, updated_at = NOW() \
             WHERE id = ? AND status = 'processing' AND locked_by = ?",
            vec![
                Value::String(Some(new_status.into())),
                Value::String(Some(next_retry.into())),
                Value::String(Some(last_error.into())),
                Value::BigUnsigned(Some(id)),
                Value::String(Some(claim_token.into())),
            ],
        );
        let result = self.db.execute_raw(stmt).await.map_err(map_db_err)?;
        Ok(result.rows_affected() > 0)
    }
    async fn list_by_aggregate(
        &self,
        aggregate_type: &str,
        aggregate_id: u64,
    ) -> Result<Vec<DomainEvent>, WebIngestionError> {
        let rows = domain_event_outbox::Entity::find()
            .filter(domain_event_outbox::Column::AggregateType.eq(aggregate_type))
            .filter(domain_event_outbox::Column::AggregateId.eq(aggregate_id))
            .order_by_asc(domain_event_outbox::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(rows.into_iter().map(model_to_event).collect())
    }
}

fn outbox_event_priority_sql() -> &'static str {
    // Prefer deeper pipeline stages before fetching/cleaning more pages. Without
    // this, a large seed import can leave thousands of pages half-processed
    // because older PageFetched events keep winning over newer PageCleaned ones.
    "CASE event_type \
        WHEN 'KnowledgePublishRequested' THEN 0 \
        WHEN 'KnowledgeRollbackRequested' THEN 0 \
        WHEN 'KnowledgeStaged' THEN 1 \
        WHEN 'DocumentIndexed' THEN 2 \
        WHEN 'ChunksEmbedded' THEN 3 \
        WHEN 'DocumentChunked' THEN 4 \
        WHEN 'QualityChecked' THEN 5 \
        WHEN 'PageDistilled' THEN 6 \
        WHEN 'PageCleaned' THEN 7 \
        WHEN 'IngestionSkipped' THEN 8 \
        WHEN 'IngestionRejected' THEN 8 \
        WHEN 'IngestionFailed' THEN 8 \
        WHEN 'IngestionDead' THEN 8 \
        WHEN 'KnowledgePublished' THEN 8 \
        WHEN 'KnowledgeSuperseded' THEN 8 \
        WHEN 'KnowledgeRolledBack' THEN 8 \
        WHEN 'PageFetched' THEN 9 \
        WHEN 'UrlDiscovered' THEN 10 \
        WHEN 'CrawlJobCreated' THEN 20 \
        ELSE 30 \
    END"
}

fn append_event_type_scope(sql: &mut String, values: &mut Vec<Value>, quota: &OutboxClaimQuota) {
    if !quota.event_types.is_empty() {
        let placeholders = std::iter::repeat("?")
            .take(quota.event_types.len())
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(" AND event_type IN (");
        sql.push_str(&placeholders);
        sql.push(')');
        values.extend(
            quota
                .event_types
                .iter()
                .map(|event_type| Value::String(Some(event_type.clone().into()))),
        );
    }

    if !quota.exclude_event_types.is_empty() {
        let placeholders = std::iter::repeat("?")
            .take(quota.exclude_event_types.len())
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(" AND event_type NOT IN (");
        sql.push_str(&placeholders);
        sql.push(')');
        values.extend(
            quota
                .exclude_event_types
                .iter()
                .map(|event_type| Value::String(Some(event_type.clone().into()))),
        );
    }
}

fn model_to_event(m: domain_event_outbox::Model) -> DomainEvent {
    DomainEvent {
        id: m.id,
        event_key: m.event_key,
        event_type: m.event_type,
        aggregate_type: m.aggregate_type,
        aggregate_id: m.aggregate_id,
        payload: m.payload,
        status: m.status,
        retry_count: m.retry_count,
        max_retries: m.max_retries,
        next_retry_at: m.next_retry_at.map(to_utc),
        locked_by: m.locked_by,
        locked_until: m.locked_until.map(to_utc),
        last_error: m.last_error,
        created_at: to_utc(m.created_at),
        updated_at: to_utc(m.updated_at),
        published_at: m.published_at.map(to_utc),
    }
}

// ============================================================================
// AuditLogRepository
// ============================================================================

pub struct AuditLogRepo {
    db: DatabaseConnection,
}

impl AuditLogRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AuditLogRepoT for AuditLogRepo {
    async fn insert(&self, log: NewAuditLog) -> Result<AuditLog, WebIngestionError> {
        let active = web_ingestion_audit_logs::ActiveModel {
            source_id: Set(log.source_id),
            source_url_id: Set(log.source_url_id),
            page_id: Set(log.page_id),
            run_id: Set(log.run_id),
            publish_record_id: Set(log.publish_record_id),
            action: Set(log.action),
            status: Set(log.status),
            message: Set(log.message),
            metadata: Set(log.metadata),
            ..Default::default()
        };
        let model = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(model_to_audit(model))
    }
    async fn list_by_run(&self, run_id: u64) -> Result<Vec<AuditLog>, WebIngestionError> {
        let rows = web_ingestion_audit_logs::Entity::find()
            .filter(web_ingestion_audit_logs::Column::RunId.eq(run_id))
            .order_by_asc(web_ingestion_audit_logs::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(rows.into_iter().map(model_to_audit).collect())
    }
    async fn list_by_publish_record(
        &self,
        publish_record_id: u64,
    ) -> Result<Vec<AuditLog>, WebIngestionError> {
        let rows = web_ingestion_audit_logs::Entity::find()
            .filter(web_ingestion_audit_logs::Column::PublishRecordId.eq(publish_record_id))
            .order_by_asc(web_ingestion_audit_logs::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(rows.into_iter().map(model_to_audit).collect())
    }
}

fn model_to_audit(m: web_ingestion_audit_logs::Model) -> AuditLog {
    AuditLog {
        id: m.id,
        source_id: m.source_id,
        source_url_id: m.source_url_id,
        page_id: m.page_id,
        run_id: m.run_id,
        publish_record_id: m.publish_record_id,
        action: m.action,
        status: m.status,
        message: m.message,
        metadata: m.metadata,
        created_at: to_utc(m.created_at),
    }
}
