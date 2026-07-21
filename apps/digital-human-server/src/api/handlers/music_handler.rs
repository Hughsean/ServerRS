use axum::{
    Json,
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;

use crate::api::MusicState;
use crate::api::dto::music_dto::{TrackDto, TrackListQuery};
use crate::api::error::ApiError as AppError;
use crate::domain::music::{MusicTrackUpdate, NewMusicTrack};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTrackRequest {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub duration: Option<u32>,
    pub file_data: String,
    pub mime_type: String,
    pub cover_image: Option<String>,
    pub lyrics: Option<String>,
    pub tags: Option<serde_json::Value>,
    pub mood_tags: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTrackRequest {
    pub title: Option<String>,
    pub artist: Option<Option<String>>,
    pub album: Option<Option<String>>,
    pub category: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub duration: Option<Option<u32>>,
    pub lyrics: Option<Option<String>>,
    pub tags: Option<Option<serde_json::Value>>,
    pub mood_tags: Option<Option<serde_json::Value>>,
    pub status: Option<i8>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminTrackListQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub category: Option<String>,
    pub search: Option<String>,
    pub status: Option<i8>,
}

pub async fn list_tracks(
    State(state): State<MusicState>,
    Query(params): Query<TrackListQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let page = params.page.unwrap_or(1).max(1) as u64;
    let page_size = params.page_size.unwrap_or(20).clamp(1, 100) as u64;

    let (tracks, total) = state
        .music
        .list_tracks(params.category, params.search, page, page_size)
        .await?;

    let items: Vec<TrackDto> = tracks.into_iter().map(track_list_item_to_dto).collect();

    Ok(Json(json!({
        "items": items,
        "total": total,
        "page": page,
        "pageSize": page_size,
    })))
}

pub async fn get_track(
    State(state): State<MusicState>,
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
        status: track.status,
    }))
}

pub async fn admin_list_tracks(
    State(state): State<MusicState>,
    Query(params): Query<AdminTrackListQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let page = params.page.unwrap_or(1).max(1) as u64;
    let page_size = params.page_size.unwrap_or(20).clamp(1, 100) as u64;
    let (tracks, total) = state
        .music
        .admin_list(
            params.category,
            params.search,
            params.status,
            page,
            page_size,
        )
        .await?;
    let items: Vec<TrackDto> = tracks.into_iter().map(track_list_item_to_dto).collect();
    Ok(Json(json!({
        "items": items,
        "total": total,
        "page": page,
        "pageSize": page_size,
    })))
}

pub async fn stream_track(
    State(state): State<MusicState>,
    Path(id): Path<u64>,
) -> Result<Response, AppError> {
    let (data, mime_type, file_size) = state.music.stream_track(id).await?;

    let headers = [
        (header::CONTENT_TYPE, mime_type),
        (header::CONTENT_LENGTH, file_size.to_string()),
        (header::CONTENT_DISPOSITION, "inline".to_string()),
    ];

    Ok((headers, data).into_response())
}

pub async fn admin_create_track(
    State(state): State<MusicState>,
    Json(payload): Json<CreateTrackRequest>,
) -> Result<Json<TrackDto>, AppError> {
    let file_bytes = decode_base64(&payload.file_data, "fileData")?;
    let cover_image = match payload.cover_image {
        Some(value) => Some(decode_base64(&value, "coverImage")?),
        None => None,
    };

    let track = state
        .music
        .admin_create(NewMusicTrack {
            title: payload.title,
            artist: payload.artist,
            album: payload.album,
            category: payload.category,
            description: payload.description,
            duration: payload.duration,
            file_data: payload.file_data,
            file_size: file_bytes.len() as u64,
            mime_type: payload.mime_type,
            cover_image,
            lyrics: payload.lyrics,
            tags: payload.tags,
            mood_tags: payload.mood_tags,
        })
        .await?;

    Ok(Json(track_to_dto(track)))
}

pub async fn admin_update_track(
    State(state): State<MusicState>,
    Path(id): Path<u64>,
    Json(payload): Json<UpdateTrackRequest>,
) -> Result<Json<TrackDto>, AppError> {
    let track = state
        .music
        .admin_update(
            id,
            MusicTrackUpdate {
                title: payload.title,
                artist: payload.artist,
                album: payload.album,
                category: payload.category,
                description: payload.description,
                duration: payload.duration,
                lyrics: payload.lyrics,
                tags: payload.tags,
                mood_tags: payload.mood_tags,
                status: payload.status,
            },
        )
        .await?;

    Ok(Json(track_to_dto(track)))
}

pub async fn admin_delete_track(
    State(state): State<MusicState>,
    Path(id): Path<u64>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.music.admin_delete(id).await?;
    Ok(Json(json!({ "deleted": true })))
}

fn decode_base64(value: &str, field: &str) -> Result<Vec<u8>, AppError> {
    base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .map_err(|_| AppError::Validation(format!("{field} must be base64-encoded")))
}

fn track_to_dto(track: crate::domain::music::MusicTrack) -> TrackDto {
    TrackDto {
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
        status: track.status,
    }
}

fn track_list_item_to_dto(track: crate::domain::music::MusicTrackListItem) -> TrackDto {
    TrackDto {
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
        status: track.status,
    }
}
