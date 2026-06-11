use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::shared::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageBackend {
    Local,
    S3,
    Minio,
}

impl fmt::Display for StorageBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageBackend::Local => write!(f, "Local"),
            StorageBackend::S3 => write!(f, "S3"),
            StorageBackend::Minio => write!(f, "Minio"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoredObject {
    pub id: u64,
    pub bucket: String,
    pub object_key: String,
    pub original_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub storage_backend: StorageBackend,
    pub public_url: Option<String>,
    pub created_by: Option<u64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PutObjectInput {
    pub bucket: String,
    pub original_name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
    pub created_by: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ObjectBytes {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub original_name: String,
}

#[async_trait]
pub trait ObjectStorage: Send + Sync {
    async fn put(&self, input: PutObjectInput) -> Result<StoredObject, AppError>;
    async fn get(&self, object_id: u64) -> Result<ObjectBytes, AppError>;
    async fn delete(&self, object_id: u64) -> Result<(), AppError>;
    async fn get_metadata(&self, object_id: u64) -> Result<StoredObject, AppError>;
}

#[async_trait]
pub trait StoredObjectRepository: Send + Sync {
    async fn save(&self, object: StoredObject) -> Result<StoredObject, AppError>;
    async fn find_by_id(&self, id: u64) -> Result<Option<StoredObject>, AppError>;
    async fn delete_by_id(&self, id: u64) -> Result<(), AppError>;
    async fn find_by_sha256(&self, sha256: &str) -> Result<Option<StoredObject>, AppError>;
}
