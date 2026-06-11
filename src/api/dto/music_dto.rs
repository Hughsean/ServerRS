use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackListQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub category: Option<String>,
    pub search: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDto {
    pub music_id: u64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub duration: Option<u32>,
    pub file_size: u64,
    pub mime_type: String,
    pub lyrics: Option<String>,
    pub tags: Option<serde_json::Value>,
    pub mood_tags: Option<serde_json::Value>,
}
