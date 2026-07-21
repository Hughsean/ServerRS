use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::domain::storage::{StorageBackend, StoredObject, StoredObjectRepoT};
use crate::shared::error::AppError;

use super::super::entities::stored_objects;

pub struct StoredObjectRepo {
    db: DatabaseConnection,
}

impl StoredObjectRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn backend_to_str(backend: &StorageBackend) -> String {
    match backend {
        StorageBackend::Local => "LOCAL",
        StorageBackend::S3 => "S3",
        StorageBackend::Minio => "MINIO",
    }
    .to_string()
}

fn str_to_backend(value: &str) -> StorageBackend {
    match value {
        "S3" => StorageBackend::S3,
        "MINIO" => StorageBackend::Minio,
        _ => StorageBackend::Local,
    }
}

fn map(m: stored_objects::Model) -> StoredObject {
    StoredObject {
        id: m.object_id,
        bucket: m.bucket,
        object_key: m.object_key,
        original_name: m.original_name,
        mime_type: m.mime_type,
        size_bytes: m.size_bytes,
        sha256: m.sha256,
        storage_backend: str_to_backend(&m.storage_backend),
        public_url: m.public_url,
        created_by: m.created_by,
        created_at: m.created_at.and_utc(),
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl StoredObjectRepoT for StoredObjectRepo {
    async fn save(&self, object: StoredObject) -> Result<StoredObject, AppError> {
        let am: stored_objects::ActiveModel = stored_objects::ActiveModel::builder()
            .set_bucket(object.bucket)
            .set_object_key(object.object_key)
            .set_original_name(object.original_name)
            .set_mime_type(object.mime_type)
            .set_size_bytes(object.size_bytes)
            .set_sha256(object.sha256)
            .set_storage_backend(backend_to_str(&object.storage_backend))
            .set_public_url(object.public_url)
            .set_created_by(object.created_by)
            .set_created_at(object.created_at.naive_utc())
            .into();

        Ok(map(am.insert(&self.db).await.map_err(map_err)?))
    }

    async fn find_by_id(&self, id: u64) -> Result<Option<StoredObject>, AppError> {
        stored_objects::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|o| o.map(map))
    }

    async fn delete_by_id(&self, id: u64) -> Result<(), AppError> {
        stored_objects::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn find_by_sha256(&self, sha256: &str) -> Result<Option<StoredObject>, AppError> {
        stored_objects::Entity::find()
            .filter(stored_objects::Column::Sha256.eq(sha256))
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|o| o.map(map))
    }
}
