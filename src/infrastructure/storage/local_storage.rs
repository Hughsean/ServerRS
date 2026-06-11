use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::domain::storage::{ObjectBytes, ObjectStorage, PutObjectInput, StoredObject};
use crate::shared::error::AppError;

pub struct LocalObjectStorage {
    base_path: std::path::PathBuf,
}

impl LocalObjectStorage {
    pub fn new(base_path: std::path::PathBuf) -> Self {
        Self { base_path }
    }

    fn sha256_hex(data: &[u8]) -> String {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
}

#[async_trait]
impl ObjectStorage for LocalObjectStorage {
    async fn put(&self, input: PutObjectInput) -> Result<StoredObject, AppError> {
        let sha256 = Self::sha256_hex(&input.data);
        let prefix = &sha256[..2];
        let dir = self.base_path.join(&input.bucket).join(prefix);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| AppError::internal(format!("failed to create storage dir: {e}")))?;

        let path = dir.join(&sha256);
        tokio::fs::write(&path, &input.data)
            .await
            .map_err(|e| AppError::internal(format!("failed to write object: {e}")))?;

        Ok(StoredObject {
            id: 0, // assigned by DB
            bucket: input.bucket,
            object_key: format!("{}/{}", prefix, sha256),
            original_name: input.original_name,
            mime_type: input.mime_type,
            size_bytes: input.data.len() as u64,
            sha256,
            storage_backend: crate::domain::storage::StorageBackend::Local,
            public_url: None,
            created_by: input.created_by,
            created_at: chrono::Utc::now(),
        })
    }

    async fn get(&self, object_id: u64) -> Result<ObjectBytes, AppError> {
        Err(AppError::NotFound(format!(
            "object {object_id}: direct filesystem lookup not yet implemented; use StoredObjectRepository first"
        )))
    }

    async fn delete(&self, object_id: u64) -> Result<(), AppError> {
        Err(AppError::NotFound(format!(
            "object {object_id}: direct filesystem delete not yet implemented; use StoredObjectRepository first"
        )))
    }

    async fn get_metadata(&self, object_id: u64) -> Result<StoredObject, AppError> {
        Err(AppError::NotFound(format!(
            "object {object_id}: metadata lookup not yet implemented; use StoredObjectRepository first"
        )))
    }
}
