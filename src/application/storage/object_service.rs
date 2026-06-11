use std::sync::Arc;

use crate::domain::storage::{ObjectBytes, ObjectStorage, PutObjectInput, StoredObject};
use crate::shared::config::StorageConfig;
use crate::shared::error::AppError;

pub struct ObjectService {
    storage: Arc<dyn ObjectStorage>,
    config: StorageConfig,
}

impl ObjectService {
    pub fn new(storage: Arc<dyn ObjectStorage>, config: StorageConfig) -> Self {
        Self { storage, config }
    }

    pub async fn upload(
        &self,
        created_by: Option<u64>,
        bucket: String,
        original_name: String,
        mime_type: String,
        data: Vec<u8>,
    ) -> Result<StoredObject, AppError> {
        // Validate size based on bucket
        let max_bytes = match bucket.as_str() {
            "avatar" => self.config.max_avatar_bytes,
            "image" | "community" => self.config.max_image_bytes,
            "audio" | "music" => self.config.max_audio_bytes,
            "document" => self.config.max_document_bytes,
            "video" => self.config.max_video_bytes,
            _ => self.config.max_document_bytes,
        };
        if data.len() as u64 > max_bytes {
            return Err(AppError::Validation(format!(
                "file too large: {} bytes (max {} bytes)",
                data.len(),
                max_bytes
            )));
        }

        let input = PutObjectInput {
            bucket,
            original_name,
            mime_type,
            data,
            created_by,
        };
        self.storage.put(input).await
    }

    pub async fn get_bytes(&self, object_id: u64) -> Result<ObjectBytes, AppError> {
        self.storage.get(object_id).await
    }

    pub async fn get_metadata(&self, object_id: u64) -> Result<StoredObject, AppError> {
        self.storage.get_metadata(object_id).await
    }

    pub async fn delete(&self, _user_id: u64, object_id: u64) -> Result<(), AppError> {
        // TODO: Add reference count check (music, community, etc.)
        self.storage.delete(object_id).await
    }
}
