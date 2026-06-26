use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::event_types::{aggregate, event as ev};
use crate::domain::web_ingestion::review::{
    KnowledgeReviewAuditEntry, KnowledgeReviewDetail, KnowledgeReviewFilter, KnowledgeReviewItem,
    KnowledgeReviewPage, KnowledgeReviewRepository, NewReviewPublishRequest, ReviewPublishRequest,
};
use crate::domain::web_ingestion::status::{publish_status, run_stage, run_status};
use crate::infra::db::entities::{
    domain_event_outbox, knowledge_documents, knowledge_ingestion_runs, knowledge_publish_records,
    web_ingestion_audit_logs, web_pages, web_sources,
};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait,
};
use std::collections::HashMap;
pub struct SeaOrmKnowledgeReviewRepository {
    db: DatabaseConnection,
}
impl SeaOrmKnowledgeReviewRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
    async fn hydrate(
        &self,
        records: Vec<knowledge_publish_records::Model>,
    ) -> Result<Vec<KnowledgeReviewItem>, WebIngestionError> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let run_ids = records.iter().map(|row| row.run_id).collect::<Vec<_>>();
        let document_ids = records
            .iter()
            .map(|row| row.document_id)
            .collect::<Vec<_>>();
        let page_ids = records.iter().map(|row| row.page_id).collect::<Vec<_>>();
        let source_ids = records.iter().map(|row| row.source_id).collect::<Vec<_>>();
        let runs = knowledge_ingestion_runs::Entity::find()
            .filter(knowledge_ingestion_runs::Column::Id.is_in(run_ids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(|row| (row.id, row))
            .collect::<HashMap<_, _>>();
        let documents = knowledge_documents::Entity::find()
            .filter(knowledge_documents::Column::DocumentId.is_in(document_ids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(|row| (row.document_id, row))
            .collect::<HashMap<_, _>>();
        let pages = web_pages::Entity::find()
            .filter(web_pages::Column::Id.is_in(page_ids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(|row| (row.id, row))
            .collect::<HashMap<_, _>>();
        let sources = web_sources::Entity::find()
            .filter(web_sources::Column::Id.is_in(source_ids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(|row| (row.id, row))
            .collect::<HashMap<_, _>>();
        records
            .into_iter()
            .map(|record| {
                let run = runs.get(&record.run_id).ok_or_else(|| {
                    WebIngestionError::Internal(format!(
                        "review record {} references missing run {}",
                        record.id, record.run_id
                    ))
                })?;
                let document = documents.get(&record.document_id).ok_or_else(|| {
                    WebIngestionError::Internal(format!(
                        "review record {} references missing document {}",
                        record.id, record.document_id
                    ))
                })?;
                let page = pages.get(&record.page_id).ok_or_else(|| {
                    WebIngestionError::Internal(format!(
                        "review record {} references missing page {}",
                        record.id, record.page_id
                    ))
                })?;
                let source = sources.get(&record.source_id).ok_or_else(|| {
                    WebIngestionError::Internal(format!(
                        "review record {} references missing source {}",
                        record.id, record.source_id
                    ))
                })?;
                Ok(KnowledgeReviewItem {
                    publish_record_id: record.id,
                    source_id: record.source_id,
                    source_name: source.name.clone(),
                    page_id: record.page_id,
                    run_id: record.run_id,
                    document_id: record.document_id,
                    version_key: record.version_key,
                    title: document.title.clone(),
                    source_url: page
                        .canonical_url
                        .clone()
                        .unwrap_or_else(|| page.url.clone()),
                    publish_status: record.publish_status,
                    active: record.active != 0,
                    run_status: run.status.clone(),
                    run_stage: run.stage.clone(),
                    quality_score: run.quality_score,
                    quality_result: run.quality_result.clone(),
                    risk_flags: run.risk_flags.clone(),
                    should_publish: run.should_publish.map(|value| value != 0),
                    created_at: to_utc(record.created_at),
                    updated_at: to_utc(record.updated_at),
                })
            })
            .collect()
    }
}
#[async_trait]
impl KnowledgeReviewRepository for SeaOrmKnowledgeReviewRepository {
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
            .order_by_desc(knowledge_publish_records::Column::CreatedAt)
            .paginate(&self.db, filter.page_size);
        let total = paginator.num_items().await.map_err(map_db_err)?;
        let records = paginator
            .fetch_page(filter.page.saturating_sub(1))
            .await
            .map_err(map_db_err)?;
        Ok(KnowledgeReviewPage {
            items: self.hydrate(records).await?,
            page: filter.page,
            page_size: filter.page_size,
            total,
        })
    }
    async fn find_item_by_id(
        &self,
        publish_record_id: u64,
    ) -> Result<Option<KnowledgeReviewItem>, WebIngestionError> {
        let record = knowledge_publish_records::Entity::find_by_id(publish_record_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        let Some(record) = record else {
            return Ok(None);
        };
        Ok(self.hydrate(vec![record]).await?.into_iter().next())
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
            .hydrate(vec![record])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| WebIngestionError::Internal("review hydration failed".into()))?;
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
        let event = domain_event_outbox::ActiveModel {            event_key: Set(request.event_key),            event_type: Set(ev::KNOWLEDGE_PUBLISH_REQUESTED.into()),            aggregate_type: Set(aggregate::KNOWLEDGE_PUBLISH_RECORD.into()),            aggregate_id: Set(record.id),            payload: Set(serde_json::json!({                "publish_record_id": record.id,                "run_id": record.run_id,                "automatic": false,                "reviewed": true,                "reviewed_by_user_id": request.reviewer_user_id,                "reviewed_by_username": request.reviewer_username.clone(),                "review_notes": request.notes.clone(),            })),            status: Set("pending".into()),            max_retries: Set(5),            ..Default::default()        }        .insert(&txn)        .await        .map_err(map_db_err)?;
        web_ingestion_audit_logs::ActiveModel {            source_id: Set(Some(record.source_id)),            source_url_id: Set(run.source_url_id),            page_id: Set(Some(record.page_id)),            run_id: Set(Some(record.run_id)),            publish_record_id: Set(Some(record.id)),            action: Set("manual_publish_requested".into()),            status: Set("pending".into()),            message: Set(format!(                "reviewer {} requested publication",                request.reviewer_username            )),            metadata: Set(Some(serde_json::json!({                "reviewed_by_user_id": request.reviewer_user_id,                "reviewed_by_username": request.reviewer_username,                "review_notes": request.notes,                "event_id": event.id,            }))),            ..Default::default()        }        .insert(&txn)        .await        .map_err(map_db_err)?;
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
