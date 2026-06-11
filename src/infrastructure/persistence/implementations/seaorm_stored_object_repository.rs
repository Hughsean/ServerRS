use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::domain::storage::{StorageBackend, StoredObject, StoredObjectRepository};
use crate::shared::error::AppError;

use super::super::entities::stored_objects;

pub struct SeaOrmStoredObjectRepository {
    db: DatabaseConnection,
}

impl SeaOrmStoredObjectRepository {
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
        created_at: m.created_at,
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

#[async_trait]
impl StoredObjectRepository for SeaOrmStoredObjectRepository {
    async fn save(&self, object: StoredObject) -> Result<StoredObject, AppError> {
        let am = stored_objects::ActiveModel {
            bucket: Set(object.bucket),
            object_key: Set(object.object_key),
            original_name: Set(object.original_name),
            mime_type: Set(object.mime_type),
            size_bytes: Set(object.size_bytes),
            sha256: Set(object.sha256),
            storage_backend: Set(backend_to_str(&object.storage_backend)),
            public_url: Set(object.public_url),
            created_by: Set(object.created_by),
            created_at: Set(object.created_at),
            ..Default::default()
        };

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
