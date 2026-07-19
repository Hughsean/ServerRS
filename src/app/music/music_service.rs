use std::sync::Arc;

use base64::Engine;

use crate::domain::music::{
    MusicRepoT, MusicTrack, MusicTrackListItem, MusicTrackUpdate, NewMusicTrack,
};
use crate::shared::error::AppError;

/// Music service — no longer uses ObjectStorage/object_id.
/// The current `music` table stores file_data / file_size / mime_type / cover_image directly.
pub struct MusicService {
    pub repo: Arc<dyn MusicRepoT>,
}

impl MusicService {
    pub fn new(repo: Arc<dyn MusicRepoT>) -> Self {
        Self { repo }
    }

    pub async fn list_tracks(
        &self,
        category: Option<String>,
        search: Option<String>,
        page: u64,
        size: u64,
    ) -> Result<(Vec<MusicTrackListItem>, u64), AppError> {
        let offset = page.saturating_sub(1) * size;
        self.repo.find_all(category, search, size, offset).await
    }

    pub async fn count_all(&self) -> Result<u64, AppError> {
        self.repo.count_all().await
    }

    pub async fn count_trend(&self, days: u32) -> Result<Vec<(String, u64)>, AppError> {
        self.repo.count_trend(days).await
    }

    pub async fn get_track(&self, id: u64) -> Result<MusicTrack, AppError> {
        let track = self
            .repo
            .find_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("track {id} not found")))?;
        if track.status != 1 {
            return Err(AppError::NotFound(format!("track {id} not found")));
        }
        Ok(track)
    }

    /// Returns the raw file data and MIME type for streaming.
    pub async fn stream_track(&self, id: u64) -> Result<(Vec<u8>, String, u64), AppError> {
        let track = self.get_track(id).await?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(track.file_data.trim())
            .map_err(|_| AppError::Internal(format!("track {id} contains invalid audio data")))?;
        let size = data.len() as u64;
        Ok((data, track.mime_type, size))
    }

    pub async fn admin_create(&self, new: NewMusicTrack) -> Result<MusicTrack, AppError> {
        validate_title_and_mime(&new.title, &new.mime_type)?;
        self.repo.save(new).await
    }

    pub async fn admin_list(
        &self,
        category: Option<String>,
        search: Option<String>,
        status: Option<i8>,
        page: u64,
        size: u64,
    ) -> Result<(Vec<MusicTrackListItem>, u64), AppError> {
        if status.is_some_and(|value| value != 0 && value != 1) {
            return Err(AppError::Validation("music status must be 0 or 1".into()));
        }
        let page = page.max(1);
        let size = size.clamp(1, 100);
        self.repo
            .find_all_admin(category, search, status, size, (page - 1) * size)
            .await
    }

    pub async fn admin_update(
        &self,
        id: u64,
        update: MusicTrackUpdate,
    ) -> Result<MusicTrack, AppError> {
        if let Some(title) = update.title.as_deref() {
            if title.trim().is_empty() {
                return Err(AppError::Validation("music title cannot be empty".into()));
            }
        }
        if update.status.is_some_and(|value| value != 0 && value != 1) {
            return Err(AppError::Validation("music status must be 0 or 1".into()));
        }
        self.repo
            .find_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("track {id} not found")))?;
        self.repo.update(id, update).await
    }

    pub async fn admin_delete(&self, id: u64) -> Result<(), AppError> {
        if self.repo.delete_by_id(id).await? {
            Ok(())
        } else {
            Err(AppError::NotFound(format!("track {id} not found")))
        }
    }
}

fn validate_title_and_mime(title: &str, mime_type: &str) -> Result<(), AppError> {
    if title.trim().is_empty() {
        return Err(AppError::Validation("music title cannot be empty".into()));
    }
    if !mime_type.to_ascii_lowercase().starts_with("audio/") {
        return Err(AppError::Validation(
            "music mimeType must be an audio MIME type".into(),
        ));
    }
    Ok(())
}
