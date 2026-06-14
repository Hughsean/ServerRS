use std::sync::Arc;

use crate::domain::storage::{
    ObjectBytes, ObjectStorage, PutObjectInput, StoredObject, StoredObjectRepository,
};
use crate::shared::config::StorageConfig;
use crate::shared::error::AppError;

pub struct ObjectService {
    storage: Arc<dyn ObjectStorage>,
    repo: Arc<dyn StoredObjectRepository>,
    config: StorageConfig,
}

impl ObjectService {
    pub fn new(
        storage: Arc<dyn ObjectStorage>,
        repo: Arc<dyn StoredObjectRepository>,
        config: StorageConfig,
    ) -> Self {
        Self {
            storage,
            repo,
            config,
        }
    }

    pub fn max_upload_bytes(&self) -> usize {
        [
            self.config.max_avatar_bytes,
            self.config.max_image_bytes,
            self.config.max_audio_bytes,
            self.config.max_document_bytes,
            self.config.max_video_bytes,
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
        .saturating_add(1024 * 1024)
        .min(usize::MAX as u64) as usize
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
        let stored = self.storage.put(input).await?;
        match self.repo.save(stored.clone()).await {
            Ok(object) => Ok(object),
            Err(e) => {
                if let Err(delete_err) = self.storage.delete(&stored).await {
                    tracing::warn!(error = %delete_err, "failed to clean up object after metadata save failed");
                }
                Err(e)
            }
        }
    }

    pub async fn get_bytes(&self, user_id: u64, object_id: u64) -> Result<ObjectBytes, AppError> {
        let object = self
            .repo
            .find_by_id(object_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("object {object_id} not found")))?;
        ensure_owner(&object, user_id)?;
        self.storage.get(&object).await
    }

    pub async fn get_metadata(
        &self,
        user_id: u64,
        object_id: u64,
    ) -> Result<StoredObject, AppError> {
        let object = self
            .repo
            .find_by_id(object_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("object {object_id} not found")))?;
        ensure_owner(&object, user_id)?;
        Ok(object)
    }

    pub async fn delete(&self, user_id: u64, object_id: u64) -> Result<(), AppError> {
        let object = self
            .repo
            .find_by_id(object_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("object {object_id} not found")))?;

        ensure_owner(&object, user_id)?;

        self.storage.delete(&object).await?;
        self.repo.delete_by_id(object_id).await
    }
}

fn ensure_owner(object: &StoredObject, user_id: u64) -> Result<(), AppError> {
    if object.created_by == Some(user_id) {
        Ok(())
    } else {
        Err(AppError::Forbidden("not your object".into()))
    }
}
