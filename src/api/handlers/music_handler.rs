use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::api::ApiState;
use crate::api::dto::music_dto::{TrackDto, TrackListQuery};
use crate::shared::error::AppError;

pub async fn list_tracks(
    State(state): State<ApiState>,
    Query(params): Query<TrackListQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let page = params.page.unwrap_or(1).max(1) as u64;
    let page_size = params.page_size.unwrap_or(20).clamp(1, 100) as u64;

    let (tracks, total) = state
        .music
        .list_tracks(params.category, params.search, page, page_size)
        .await?;

    let items: Vec<TrackDto> = tracks
        .into_iter()
        .map(|t| TrackDto {
            music_id: t.music_id,
            title: t.title,
            artist: t.artist,
            album: t.album,
            category: t.category,
            description: t.description,
            duration: t.duration,
            file_size: t.file_size,
            mime_type: t.mime_type,
            lyrics: t.lyrics,
            tags: t.tags,
            mood_tags: t.mood_tags,
        })
        .collect();

    Ok(Json(json!({
        "items": items,
        "total": total,
        "page": page,
        "pageSize": page_size,
    })))
}

pub async fn get_track(
    State(state): State<ApiState>,
    Path(id): Path<u64>,
) -> Result<Json<TrackDto>, AppError> {
    let track = state.music.get_track(id).await?;

    Ok(Json(TrackDto {
        music_id: track.music_id,
        title: track.title,
        artist: track.artist,
        album: track.album,
        category: track.category,
        description: track.description,
        duration: track.duration,
        file_size: track.file_size,
        mime_type: track.mime_type,
        lyrics: track.lyrics,
        tags: track.tags,
        mood_tags: track.mood_tags,
    }))
}

pub async fn stream_track(
    State(state): State<ApiState>,
    Path(id): Path<u64>,
) -> Result<Response, AppError> {
    let (data, mime_type, file_size) = state.music.stream_track(id).await?;

    let headers = [
        (header::CONTENT_TYPE, mime_type),
        (header::CONTENT_LENGTH, file_size.to_string()),
        (
            header::CONTENT_DISPOSITION,
            "inline".to_string(),
        ),
    ];

    Ok((headers, data).into_response())
}
