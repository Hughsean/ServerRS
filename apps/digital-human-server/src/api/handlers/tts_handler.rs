use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::api::dto::tts_dto::SignedAudioQuery;
use crate::api::error::ApiError as AppError;
use crate::api::state::TtsState;

/// GET /api/v1/tts/audio/{file_id}
///
/// URL 签名无效、过期或文件不存在统一返回 404，避免暴露文件存在性。
pub async fn get_signed_audio(
    State(state): State<TtsState>,
    Path(file_id): Path<String>,
    Query(query): Query<SignedAudioQuery>,
) -> Result<Response, AppError> {
    let Some(tts) = state.tts else {
        return Err(AppError::NotFound("语音文件不存在或已失效".into()));
    };
    let file_id = Uuid::parse_str(&file_id)
        .map_err(|_| AppError::NotFound("语音文件不存在或已失效".into()))?;
    let Some((path, mime_type)) = tts.resolve_signed_file(file_id, query.expires, &query.signature)
    else {
        return Err(AppError::NotFound("语音文件不存在或已失效".into()));
    };
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| AppError::NotFound("语音文件不存在或已失效".into()))?;
    let body = Body::from_stream(ReaderStream::new(file));
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, mime_type)], body).into_response())
}
