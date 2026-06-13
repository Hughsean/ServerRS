use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::shared::error::AppError;

/// Matches the `music` table entity. Uses file_data / file_size / mime_type / cover_image
/// for BLOB storage (no stored_objects/object_id in current DB).
/// PK is music_id, duration is Option<u32>, status is i8 (1=active).
#[derive(Debug, Clone, Serialize)]
pub struct MusicTrack {
    pub music_id: u64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub duration: Option<u32>,
    pub file_data: String,
    pub file_size: u64,
    pub mime_type: String,
    pub cover_image: Option<Vec<u8>>,
    pub lyrics: Option<String>,
    pub tags: Option<serde_json::Value>,
    pub mood_tags: Option<serde_json::Value>,
    pub status: i8,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewMusicTrack {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub duration: Option<u32>,
    pub file_data: String,
    pub file_size: u64,
    pub mime_type: String,
    pub cover_image: Option<Vec<u8>>,
    pub lyrics: Option<String>,
    pub tags: Option<serde_json::Value>,
    pub mood_tags: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct MusicTrackUpdate {
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

#[async_trait]
pub trait MusicRepository: Send + Sync {
    async fn save(&self, track: NewMusicTrack) -> Result<MusicTrack, AppError>;
    async fn find_by_id(&self, id: u64) -> Result<Option<MusicTrack>, AppError>;
    async fn find_all(
        &self,
        category: Option<String>,
        search: Option<String>,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<MusicTrack>, u64), AppError>;
    async fn find_all_admin(
        &self,
        category: Option<String>,
        search: Option<String>,
        status: Option<i8>,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<MusicTrack>, u64), AppError>;
    async fn update(&self, id: u64, update: MusicTrackUpdate) -> Result<MusicTrack, AppError>;
    async fn delete_by_id(&self, id: u64) -> Result<bool, AppError>;
}
