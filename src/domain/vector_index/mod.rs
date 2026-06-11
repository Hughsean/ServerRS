use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::shared::error::AppError;

/// A record tracking the indexing status of an object in the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorIndexRecord {
    pub record_id: u64,
    pub vector_id: String,
    pub collection_name: String,
    pub object_type: String,
    pub object_id: u64,
    pub owner_user_id: Option<u64>,
    pub source_table: String,
    pub source_hash: Option<String>,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_dimension: u32,
    pub payload: serde_json::Value,
    pub index_status: String,
    pub indexed_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewVectorIndexRecord {
    pub vector_id: String,
    pub collection_name: String,
    pub object_type: String,
    pub object_id: u64,
    pub owner_user_id: Option<u64>,
    pub source_table: String,
    pub source_hash: Option<String>,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_dimension: u32,
    pub payload: serde_json::Value,
    pub index_status: String,
}

/// An async job for index management (upsert, delete, rebuild).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorIndexJob {
    pub job_id: u64,
    pub action: String,
    pub object_type: String,
    pub object_id: u64,
    pub collection_name: String,
    pub vector_id: Option<String>,
    pub priority: i32,
    pub status: String,
    pub attempts: u32,
    pub max_attempts: u32,
    pub next_run_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewVectorIndexJob {
    pub action: String,
    pub object_type: String,
    pub object_id: u64,
    pub collection_name: String,
    pub vector_id: Option<String>,
    pub priority: i32,
}

/// Repository for vector index metadata and job queue.
#[async_trait]
pub trait VectorIndexRepository: Send + Sync {
    async fn upsert_record(
        &self,
        record: NewVectorIndexRecord,
    ) -> Result<VectorIndexRecord, AppError>;

    async fn mark_indexed(
        &self,
        vector_id: &str,
        embedding_dimension: u32,
        payload: serde_json::Value,
    ) -> Result<(), AppError>;

    async fn mark_failed(&self, vector_id: &str, error_message: String) -> Result<(), AppError>;

    async fn mark_deleted(&self, vector_id: &str) -> Result<(), AppError>;

    async fn find_by_vector_id(
        &self,
        vector_id: &str,
    ) -> Result<Option<VectorIndexRecord>, AppError>;

    async fn list_stale_by_collection(
        &self,
        collection_name: &str,
        limit: u64,
    ) -> Result<Vec<VectorIndexRecord>, AppError>;

    async fn enqueue_job(&self, job: NewVectorIndexJob) -> Result<VectorIndexJob, AppError>;

    async fn fetch_pending_jobs(
        &self,
        limit: u64,
        worker_id: &str,
    ) -> Result<Vec<VectorIndexJob>, AppError>;

    async fn mark_job_succeeded(&self, job_id: u64) -> Result<(), AppError>;

    async fn mark_job_failed(
        &self,
        job_id: u64,
        error_message: String,
        retry: bool,
    ) -> Result<(), AppError>;
}
