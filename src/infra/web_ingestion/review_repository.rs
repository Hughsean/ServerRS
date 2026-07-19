use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::event_types::{aggregate, event as ev};
use crate::domain::web_ingestion::review::{
    KnowledgeReviewAuditEntry, KnowledgeReviewDetail, KnowledgeReviewFilter, KnowledgeReviewItem,
    KnowledgeReviewPage, KnowledgeReviewRepoT, NewReviewPublishRequest, ReviewPublishRequest,
};
use crate::domain::web_ingestion::status::{publish_status, run_stage, run_status};
use crate::infra::repo::entities::{
    domain_event_outbox, knowledge_documents, knowledge_ingestion_runs, knowledge_publish_records,
    web_ingestion_audit_logs, web_pages, web_sources,
};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DerivePartialModel,
    EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};

#[derive(DerivePartialModel)]
#[sea_orm(entity = "knowledge_ingestion_runs::Entity")]
struct KnowledgeReviewRunRow {
    status: String,
    stage: String,
    quality_score: Option<f64>,
    quality_result: Option<sea_orm::prelude::Json>,
    risk_flags: Option<sea_orm::prelude::Json>,
    should_publish: Option<i8>,
}

#[derive(DerivePartialModel)]
#[sea_orm(entity = "knowledge_documents::Entity")]
struct KnowledgeReviewDocumentRow {
    title: Option<String>,
}

#[derive(DerivePartialModel)]
#[sea_orm(entity = "web_pages::Entity")]
struct KnowledgeReviewPageRow {
    url: String,
    canonical_url: Option<String>,
}

#[derive(DerivePartialModel)]
#[sea_orm(entity = "web_sources::Entity")]
struct KnowledgeReviewSourceRow {
    name: String,
}

#[derive(DerivePartialModel)]
#[sea_orm(entity = "knowledge_publish_records::Entity")]
struct KnowledgeReviewRow {
    #[sea_orm(from_col = "id")]
    publish_record_id: u64,
    source_id: u64,
    page_id: u64,
    run_id: u64,
    document_id: u64,
    version_key: String,
    publish_status: String,
    active: i8,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
    #[sea_orm(nested)]
    run: Option<KnowledgeReviewRunRow>,
    #[sea_orm(nested)]
    document: Option<KnowledgeReviewDocumentRow>,
    #[sea_orm(nested)]
    page: Option<KnowledgeReviewPageRow>,
    #[sea_orm(nested)]
    source: Option<KnowledgeReviewSourceRow>,
}

pub struct KnowledgeReviewRepo {
    db: DatabaseConnection,
}
impl KnowledgeReviewRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn map_review_row(row: KnowledgeReviewRow) -> Result<KnowledgeReviewItem, WebIngestionError> {
    let run = row.run.ok_or_else(|| {
        WebIngestionError::Internal(format!(
            "review record {} references missing run {}",
            row.publish_record_id, row.run_id
        ))
    })?;
    let document = row.document.ok_or_else(|| {
        WebIngestionError::Internal(format!(
            "review record {} references missing document {}",
            row.publish_record_id, row.document_id
        ))
    })?;
    let page = row.page.ok_or_else(|| {
        WebIngestionError::Internal(format!(
            "review record {} references missing page {}",
            row.publish_record_id, row.page_id
        ))
    })?;
    let source = row.source.ok_or_else(|| {
        WebIngestionError::Internal(format!(
            "review record {} references missing source {}",
            row.publish_record_id, row.source_id
        ))
    })?;

    Ok(KnowledgeReviewItem {
        publish_record_id: row.publish_record_id,
        source_id: row.source_id,
        source_name: source.name,
        page_id: row.page_id,
        run_id: row.run_id,
        document_id: row.document_id,
        version_key: row.version_key,
        title: document.title,
        source_url: page.canonical_url.unwrap_or(page.url),
        publish_status: row.publish_status,
        active: row.active != 0,
        run_status: run.status,
        run_stage: run.stage,
        quality_score: run.quality_score,
        quality_result: run.quality_result.map(Into::into),
        risk_flags: run.risk_flags.map(Into::into),
        should_publish: run.should_publish.map(|value| value != 0),
        created_at: to_utc(row.created_at),
        updated_at: to_utc(row.updated_at),
    })
}
#[async_trait]
impl KnowledgeReviewRepoT for KnowledgeReviewRepo {
    async fn list(
        &self,
        filter: KnowledgeReviewFilter,
    ) -> Result<KnowledgeReviewPage, WebIngestionError> {
        let mut query = knowledge_publish_records::Entity::find();
        if filter.publish_status != "all" {
            query = query.filter(
                knowledge_publish_records::Column::PublishStatus.eq(filter.publish_status.clone()),
            );
        }
        if let Some(source_id) = filter.source_id {
            query = query.filter(knowledge_publish_records::Column::SourceId.eq(source_id));
        }
        let paginator = query
            .left_join(knowledge_ingestion_runs::Entity)
            .left_join(knowledge_documents::Entity)
            .left_join(web_pages::Entity)
            .left_join(web_sources::Entity)
            .order_by_desc(knowledge_publish_records::Column::CreatedAt)
            .into_partial_model::<KnowledgeReviewRow>()
            .paginate(&self.db, filter.page_size);
        let total = paginator.num_items().await.map_err(map_db_err)?;
        let rows = paginator
            .fetch_page(filter.page.saturating_sub(1))
            .await
            .map_err(map_db_err)?;
        Ok(KnowledgeReviewPage {
            items: rows
                .into_iter()
                .map(map_review_row)
                .collect::<Result<_, _>>()?,
            page: filter.page,
            page_size: filter.page_size,
            total,
        })
    }
    async fn find_item_by_id(
        &self,
        publish_record_id: u64,
    ) -> Result<Option<KnowledgeReviewItem>, WebIngestionError> {
        let row = knowledge_publish_records::Entity::find_by_id(publish_record_id)
            .left_join(knowledge_ingestion_runs::Entity)
            .left_join(knowledge_documents::Entity)
            .left_join(web_pages::Entity)
            .left_join(web_sources::Entity)
            .into_partial_model::<KnowledgeReviewRow>()
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        row.map(map_review_row).transpose()
    }
    async fn find_detail_by_id(
        &self,
        publish_record_id: u64,
    ) -> Result<Option<KnowledgeReviewDetail>, WebIngestionError> {
        let record = knowledge_publish_records::Entity::find_by_id(publish_record_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        let Some(record) = record else {
            return Ok(None);
        };
        let run = knowledge_ingestion_runs::Entity::find_by_id(record.run_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_ingestion_run".into(),
                id: record.run_id,
            })?;
        let audit_logs = web_ingestion_audit_logs::Entity::find()
            .filter(web_ingestion_audit_logs::Column::PublishRecordId.eq(record.id))
            .order_by_asc(web_ingestion_audit_logs::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(|log| KnowledgeReviewAuditEntry {
                action: log.action,
                status: log.status,
                message: log.message,
                metadata: log.metadata,
                created_at: to_utc(log.created_at),
            })
            .collect();
        let review = self
            .find_item_by_id(publish_record_id)
            .await?
            .ok_or_else(|| WebIngestionError::Internal("review query failed".into()))?;
        Ok(Some(KnowledgeReviewDetail {
            review,
            clean_text: run.clean_text,
            distilled_json: run.distilled_json,
            audit_logs,
        }))
    }
    async fn request_publish(
        &self,
        request: NewReviewPublishRequest,
    ) -> Result<ReviewPublishRequest, WebIngestionError> {
        let txn = self.db.begin().await.map_err(map_db_err)?;
        let record = knowledge_publish_records::Entity::find_by_id(request.publish_record_id)
            .lock_exclusive()
            .one(&txn)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_publish_record".into(),
                id: request.publish_record_id,
            })?;
        if let Some(existing) = domain_event_outbox::Entity::find()
            .filter(domain_event_outbox::Column::EventKey.eq(&request.event_key))
            .one(&txn)
            .await
            .map_err(map_db_err)?
        {
            txn.commit().await.map_err(map_db_err)?;
            return Ok(ReviewPublishRequest {
                publish_record_id: record.id,
                event_id: existing.id,
                event_status: existing.status,
                already_requested: true,
            });
        }
        if record.publish_status != publish_status::STAGED || record.active != 0 {
            return Err(WebIngestionError::ReviewConflict {
                reason: format!("publish record {} is not staged", record.id),
            });
        }
        let run = knowledge_ingestion_runs::Entity::find_by_id(record.run_id)
            .one(&txn)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| WebIngestionError::NotFound {
                entity: "knowledge_ingestion_run".into(),
                id: record.run_id,
            })?;
        if run.status != run_status::STAGED || run.stage != run_stage::STAGING {
            return Err(WebIngestionError::ReviewConflict {
                reason: format!(
                    "ingestion run {} is in ({},{})",
                    run.id, run.status, run.stage
                ),
            });
        }
        let event_active: domain_event_outbox::ActiveModel =
            domain_event_outbox::ActiveModel::builder()
                .set_event_key(request.event_key)
                .set_event_type(ev::KNOWLEDGE_PUBLISH_REQUESTED)
                .set_aggregate_type(aggregate::KNOWLEDGE_PUBLISH_RECORD)
                .set_aggregate_id(record.id)
                .set_payload(serde_json::json!({
                    "publish_record_id": record.id,
                    "run_id": record.run_id,
                    "automatic": false,
                    "reviewed": true,
                    "reviewed_by_user_id": request.reviewer_user_id,
                    "reviewed_by_username": request.reviewer_username.clone(),
                    "review_notes": request.notes.clone(),
                }))
                .set_status("pending")
                .set_max_retries(5_u32)
                .into();
        let event = event_active.insert(&txn).await.map_err(map_db_err)?;

        let audit_active: web_ingestion_audit_logs::ActiveModel =
            web_ingestion_audit_logs::ActiveModel::builder()
                .set_source_id(Some(record.source_id))
                .set_source_url_id(run.source_url_id)
                .set_page_id(Some(record.page_id))
                .set_run_id(Some(record.run_id))
                .set_publish_record_id(Some(record.id))
                .set_action("manual_publish_requested")
                .set_status("pending")
                .set_message(format!(
                    "reviewer {} requested publication",
                    request.reviewer_username
                ))
                .set_metadata(Some(serde_json::json!({
                    "reviewed_by_user_id": request.reviewer_user_id,
                    "reviewed_by_username": request.reviewer_username,
                    "review_notes": request.notes,
                    "event_id": event.id,
                })))
                .into();
        audit_active.insert(&txn).await.map_err(map_db_err)?;
        txn.commit().await.map_err(map_db_err)?;
        Ok(ReviewPublishRequest {
            publish_record_id: record.id,
            event_id: event.id,
            event_status: event.status,
            already_requested: false,
        })
    }

    async fn count_all(&self) -> Result<u64, WebIngestionError> {
        knowledge_publish_records::Entity::find()
            .count(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn count_trend(&self, days: u32) -> Result<Vec<(String, u64)>, WebIngestionError> {
        let since = chrono::Utc::now() - chrono::Duration::days(days as i64 - 1);
        let start = since.format("%Y-%m-%d").to_string();
        let stmt = sea_orm::Statement::from_sql_and_values(
            self.db.get_database_backend(),
            r#"
            SELECT DATE(created_at) AS day, COUNT(*) AS cnt
            FROM knowledge_publish_records
            WHERE created_at >= CAST(? AS DATETIME)
            GROUP BY DATE(created_at)
            ORDER BY day
            "#,
            vec![sea_orm::Value::String(Some(start))],
        );
        let rows = self.db.query_all_raw(stmt).await.map_err(map_db_err)?;
        let mut daily: Vec<(String, u64)> = rows
            .into_iter()
            .filter_map(|row| {
                let day: String = row.try_get("", "day").ok()?;
                let cnt: i64 = row.try_get("", "cnt").ok()?;
                Some((day, cnt as u64))
            })
            .collect();
        Ok(fill_trend_daily(days, &mut daily))
    }
}

fn fill_trend_daily(days: u32, daily: &mut [(String, u64)]) -> Vec<(String, u64)> {
    daily.sort_by(|a, b| a.0.cmp(&b.0));
    let mut result = Vec::with_capacity(days as usize);
    let today = chrono::Utc::now().date_naive();
    for i in (0..days).rev() {
        let date = today - chrono::Duration::days(i as i64);
        let label = date.format("%m-%d").to_string();
        let full = date.format("%Y-%m-%d").to_string();
        let count = daily
            .iter()
            .find(|(d, _)| *d == full)
            .map(|(_, c)| *c)
            .unwrap_or(0);
        result.push((label, count));
    }
    result
}

fn map_db_err(error: sea_orm::DbErr) -> WebIngestionError {
    WebIngestionError::Internal(error.to_string())
}
fn to_utc(value: NaiveDateTime) -> DateTime<Utc> {
    value.and_utc()
}
