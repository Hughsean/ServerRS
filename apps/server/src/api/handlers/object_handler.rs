use axum::{
    Extension, Json,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::api::ObjectState;
use crate::api::error::ApiError as AppError;
use crate::app::auth::auth_service::AuthenticatedUser;

#[derive(Deserialize)]
pub struct UploadQuery {
    pub bucket: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredObjectDto {
    pub id: u64,
    pub bucket: String,
    pub object_key: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub public_url: String,
    pub created_at: DateTime<Utc>,
}

pub async fn upload_object(
    State(state): State<ObjectState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Query(query): Query<UploadQuery>,
    mut multipart: Multipart,
) -> Result<Json<StoredObjectDto>, AppError> {
    let bucket = query.bucket.unwrap_or_else(|| "default".to_string());

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Validation(e.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("upload").to_string();
        let content_type = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::Validation(e.to_string()))?;

        let result = state
            .objects
            .upload(
                Some(auth_user.user_id),
                bucket,
                filename,
                content_type,
                bytes.to_vec(),
            )
            .await?;

        return Ok(Json(StoredObjectDto {
            id: result.id,
            bucket: result.bucket,
            object_key: result.object_key,
            mime_type: result.mime_type,
            size_bytes: result.size_bytes,
            public_url: result.public_url.unwrap_or_default(),
            created_at: result.created_at,
        }));
    }

    Err(AppError::Validation(
        "missing \"file\" field in multipart body".to_string(),
    ))
}

pub async fn get_object(
    State(state): State<ObjectState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(object_id): Path<u64>,
) -> Result<impl IntoResponse, AppError> {
    let obj = state
        .objects
        .get_bytes(auth_user.user_id, object_id)
        .await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&obj.mime_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );

    Ok((headers, obj.data))
}

pub async fn get_object_metadata(
    State(state): State<ObjectState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(object_id): Path<u64>,
) -> Result<Json<StoredObjectDto>, AppError> {
    let result = state
        .objects
        .get_metadata(auth_user.user_id, object_id)
        .await?;

    Ok(Json(StoredObjectDto {
        id: result.id,
        bucket: result.bucket,
        object_key: result.object_key,
        mime_type: result.mime_type,
        size_bytes: result.size_bytes,
        public_url: result.public_url.unwrap_or_default(),
        created_at: result.created_at,
    }))
}

pub async fn delete_object(
    State(state): State<ObjectState>,
    Extension(auth_user): Extension<AuthenticatedUser>,
    Path(object_id): Path<u64>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.objects.delete(auth_user.user_id, object_id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}
