use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, Order, PaginatorTrait,
    QueryFilter, QueryOrder, Set,
};
use tracing::debug;

use super::super::entities::{vector_index_jobs, vector_index_records};

use crate::domain::vector_index::{
    NewVectorIndexJob, NewVectorIndexRecord, VectorIndexJob, VectorIndexRecord, VectorIndexRepoT,
};
use crate::shared::error::AppError;

pub struct VectorIndexRepo {
    db: DatabaseConnection,
}

impl VectorIndexRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

// ── Model → Domain mappings ───────────────────────────────────────

fn map_record(m: vector_index_records::Model) -> VectorIndexRecord {
    VectorIndexRecord {
        record_id: m.record_id,
        vector_id: m.vector_id,
        collection_name: m.collection_name,
        object_type: m.object_type,
        object_id: m.object_id,
        owner_user_id: m.owner_user_id,
        source_table: m.source_table,
        source_hash: m.source_hash,
        embedding_provider: m.embedding_provider,
        embedding_model: m.embedding_model,
        embedding_dimension: m.embedding_dimension,
        payload: m.payload.into(),
        index_status: m.index_status,
        indexed_at: m.indexed_at.map(|t| t.and_utc()),
        failed_at: m.failed_at.map(|t| t.and_utc()),
        error_message: m.error_message,
    }
}

fn map_job(m: vector_index_jobs::Model) -> VectorIndexJob {
    VectorIndexJob {
        job_id: m.job_id,
        action: m.action,
        object_type: m.object_type,
        object_id: m.object_id,
        collection_name: m.collection_name,
        vector_id: m.vector_id,
        priority: m.priority,
        status: m.status,
        attempts: m.attempts,
        max_attempts: m.max_attempts,
        next_run_at: m.next_run_at.and_utc(),
    }
}

// ── Repository implementation ──────────────────────────────────────

#[async_trait]
impl VectorIndexRepoT for VectorIndexRepo {
    async fn upsert_record(
        &self,
        record: NewVectorIndexRecord,
    ) -> Result<VectorIndexRecord, AppError> {
        let now = Utc::now().naive_utc();
        // Try to find existing record for upsert
        let existing = vector_index_records::Entity::find()
            .filter(vector_index_records::Column::VectorId.eq(&record.vector_id))
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find vector_index_record: {e}")))?;

        if let Some(model) = existing {
            let mut active: vector_index_records::ActiveModel = model.into();
            active.collection_name = Set(record.collection_name);
            active.object_type = Set(record.object_type);
            active.object_id = Set(record.object_id);
            active.owner_user_id = Set(record.owner_user_id);
            active.source_table = Set(record.source_table);
            active.source_hash = Set(record.source_hash);
            active.embedding_provider = Set(record.embedding_provider);
            active.embedding_model = Set(record.embedding_model);
            active.embedding_dimension = Set(record.embedding_dimension);
            active.payload = Set(record.payload.into());
            active.index_status = Set(record.index_status);
            active.updated_at = Set(now);
            let saved = active
                .update(&self.db)
                .await
                .map_err(|e| AppError::internal(format!("update vector_index_record: {e}")))?;
            Ok(map_record(saved))
        } else {
            let active: vector_index_records::ActiveModel =
                vector_index_records::ActiveModel::builder()
                    .set_vector_id(record.vector_id)
                    .set_collection_name(record.collection_name)
                    .set_object_type(record.object_type)
                    .set_object_id(record.object_id)
                    .set_owner_user_id(record.owner_user_id)
                    .set_source_table(record.source_table)
                    .set_source_hash(record.source_hash)
                    .set_embedding_provider(record.embedding_provider)
                    .set_embedding_model(record.embedding_model)
                    .set_embedding_dimension(record.embedding_dimension)
                    .set_payload(record.payload)
                    .set_index_status(record.index_status)
                    .set_indexed_at(None)
                    .set_failed_at(None)
                    .set_error_message(None)
                    .set_created_at(now)
                    .set_updated_at(now)
                    .into();
            let saved = active
                .insert(&self.db)
                .await
                .map_err(|e| AppError::internal(format!("insert vector_index_record: {e}")))?;
            Ok(map_record(saved))
        }
    }

    async fn mark_indexed(
        &self,
        vector_id: &str,
        embedding_dimension: u32,
        payload: serde_json::Value,
    ) -> Result<(), AppError> {
        let model = vector_index_records::Entity::find()
            .filter(vector_index_records::Column::VectorId.eq(vector_id))
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find vector_index_record: {e}")))?
            .ok_or_else(|| {
                AppError::NotFound(format!("vector_index_record {vector_id} not found"))
            })?;
        let mut active: vector_index_records::ActiveModel = model.into();
        active.index_status = Set("indexed".to_string());
        active.indexed_at = Set(Some(Utc::now().naive_utc()));
        active.embedding_dimension = Set(embedding_dimension);
        active.payload = Set(payload.into());
        active.error_message = Set(None);
        active.failed_at = Set(None);
        active.updated_at = Set(Utc::now().naive_utc());
        active
            .update(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("mark_indexed {vector_id}: {e}")))?;
        Ok(())
    }

    async fn mark_failed(&self, vector_id: &str, error_message: String) -> Result<(), AppError> {
        let now = Utc::now().naive_utc();
        let model = vector_index_records::Entity::find()
            .filter(vector_index_records::Column::VectorId.eq(vector_id))
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find vector_index_record: {e}")))?;
        match model {
            Some(m) => {
                let mut active: vector_index_records::ActiveModel = m.into();
                active.index_status = Set("failed".to_string());
                active.failed_at = Set(Some(now));
                active.error_message = Set(Some(error_message));
                active.updated_at = Set(now);
                active
                    .update(&self.db)
                    .await
                    .map_err(|e| AppError::internal(format!("mark_failed {vector_id}: {e}")))?;
            }
            None => {
                // Create a failed record even if none existed
                let active: vector_index_records::ActiveModel =
                    vector_index_records::ActiveModel::builder()
                        .set_vector_id(vector_id)
                        .set_collection_name("unknown")
                        .set_object_type("unknown")
                        .set_object_id(0_u64)
                        .set_owner_user_id(None)
                        .set_source_table("unknown")
                        .set_source_hash(None)
                        .set_embedding_provider("unknown")
                        .set_embedding_model("unknown")
                        .set_embedding_dimension(0_u32)
                        .set_payload(serde_json::Value::Null)
                        .set_index_status("failed")
                        .set_indexed_at(None)
                        .set_failed_at(Some(now))
                        .set_error_message(Some(error_message))
                        .set_created_at(now)
                        .set_updated_at(now)
                        .into();
                active.insert(&self.db).await.map_err(|e| {
                    AppError::internal(format!("insert failed record {vector_id}: {e}"))
                })?;
            }
        }
        Ok(())
    }

    async fn mark_deleted(&self, vector_id: &str) -> Result<(), AppError> {
        let model = vector_index_records::Entity::find()
            .filter(vector_index_records::Column::VectorId.eq(vector_id))
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find vector_index_record: {e}")))?;
        match model {
            Some(m) => {
                let mut active: vector_index_records::ActiveModel = m.into();
                active.index_status = Set("deleted".to_string());
                active.updated_at = Set(Utc::now().naive_utc());
                active
                    .update(&self.db)
                    .await
                    .map_err(|e| AppError::internal(format!("mark_deleted {vector_id}: {e}")))?;
            }
            None => {
                debug!(vector_id, "mark_deleted: record not found, skipping");
            }
        }
        Ok(())
    }

    async fn find_by_vector_id(
        &self,
        vector_id: &str,
    ) -> Result<Option<VectorIndexRecord>, AppError> {
        let row = vector_index_records::Entity::find()
            .filter(vector_index_records::Column::VectorId.eq(vector_id))
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find_by_vector_id: {e}")))?;
        Ok(row.map(map_record))
    }

    async fn list_stale_by_collection(
        &self,
        collection_name: &str,
        limit: u64,
    ) -> Result<Vec<VectorIndexRecord>, AppError> {
        let rows = vector_index_records::Entity::find()
            .filter(vector_index_records::Column::CollectionName.eq(collection_name))
            .filter(vector_index_records::Column::IndexStatus.is_in(["failed", "stale"]))
            .paginate(&self.db, limit)
            .fetch_page(0)
            .await
            .map_err(|e| AppError::internal(format!("list_stale_by_collection: {e}")))?;
        Ok(rows.into_iter().map(map_record).collect())
    }

    async fn enqueue_job(&self, job: NewVectorIndexJob) -> Result<VectorIndexJob, AppError> {
        let now = Utc::now().naive_utc();
        let active: vector_index_jobs::ActiveModel = vector_index_jobs::ActiveModel::builder()
            .set_action(job.action)
            .set_object_type(job.object_type)
            .set_object_id(job.object_id)
            .set_collection_name(job.collection_name)
            .set_vector_id(job.vector_id)
            .set_priority(job.priority)
            .set_status("pending")
            .set_attempts(0_u32)
            .set_max_attempts(5_u32)
            .set_next_run_at(now)
            .set_locked_at(None)
            .set_locked_by(None)
            .set_last_error(None)
            .set_created_at(now)
            .set_updated_at(now)
            .into();
        let saved = active
            .insert(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("enqueue_job: {e}")))?;
        Ok(map_job(saved))
    }

    async fn fetch_pending_jobs(
        &self,
        limit: u64,
        worker_id: &str,
    ) -> Result<Vec<VectorIndexJob>, AppError> {
        let now = Utc::now().naive_utc();
        // Simple fetch without transactional lock (full implementation would use
        // UPDATE ... WHERE with RETURNING or a two-phase approach)
        let rows = vector_index_jobs::Entity::find()
            .filter(vector_index_jobs::Column::Status.eq("pending"))
            .filter(vector_index_jobs::Column::NextRunAt.lte(now))
            .order_by(vector_index_jobs::Column::Priority, Order::Desc)
            .order_by(vector_index_jobs::Column::NextRunAt, Order::Asc)
            .paginate(&self.db, limit)
            .fetch_page(0)
            .await
            .map_err(|e| AppError::internal(format!("fetch_pending_jobs: {e}")))?;

        // Mark fetched jobs as running
        let mut results = Vec::new();
        for row in rows {
            let mut active: vector_index_jobs::ActiveModel = row.clone().into();
            active.status = Set("running".to_string());
            active.locked_at = Set(Some(now));
            active.locked_by = Set(Some(worker_id.to_string()));
            active.attempts = Set(row.attempts + 1);
            let saved = active
                .update(&self.db)
                .await
                .map_err(|e| AppError::internal(format!("lock job {}: {e}", row.job_id)))?;
            results.push(map_job(saved));
        }
        Ok(results)
    }

    async fn mark_job_succeeded(&self, job_id: u64) -> Result<(), AppError> {
        let model = vector_index_jobs::Entity::find_by_id(job_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find job {job_id}: {e}")))?
            .ok_or_else(|| AppError::NotFound(format!("job {job_id} not found")))?;
        let mut active: vector_index_jobs::ActiveModel = model.into();
        active.status = Set("succeeded".to_string());
        active.updated_at = Set(Utc::now().naive_utc());
        active
            .update(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("mark_job_succeeded {job_id}: {e}")))?;
        Ok(())
    }

    async fn mark_job_failed(
        &self,
        job_id: u64,
        error_message: String,
        retry: bool,
    ) -> Result<(), AppError> {
        let model = vector_index_jobs::Entity::find_by_id(job_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find job {job_id}: {e}")))?
            .ok_or_else(|| AppError::NotFound(format!("job {job_id} not found")))?;
        let mut active: vector_index_jobs::ActiveModel = model.clone().into();
        active.last_error = Set(Some(error_message));
        if retry && model.attempts < model.max_attempts {
            active.status = Set("pending".to_string());
            active.next_run_at = Set(Utc::now().naive_utc() + chrono::Duration::seconds(60));
        } else {
            active.status = Set(if model.attempts >= model.max_attempts {
                "dead"
            } else {
                "failed"
            }
            .to_string());
        }
        active.updated_at = Set(Utc::now().naive_utc());
        active
            .update(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("mark_job_failed {job_id}: {e}")))?;
        Ok(())
    }
}
