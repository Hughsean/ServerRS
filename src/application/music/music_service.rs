use std::sync::Arc;

use base64::Engine;

use crate::domain::music::{MusicRepository, MusicTrack, MusicTrackUpdate, NewMusicTrack};
use crate::shared::error::AppError;

/// Music service — no longer uses ObjectStorage/object_id.
/// The current `music` table stores file_data / file_size / mime_type / cover_image directly.
pub struct MusicService {
    pub repo: Arc<dyn MusicRepository>,
}

impl MusicService {
    pub fn new(repo: Arc<dyn MusicRepository>) -> Self {
        Self { repo }
    }

    pub async fn list_tracks(
        &self,
        category: Option<String>,
        search: Option<String>,
        page: u64,
        size: u64,
    ) -> Result<(Vec<MusicTrack>, u64), AppError> {
        let offset = page.saturating_sub(1) * size;
        self.repo.find_all(category, search, size, offset).await
    }

    pub async fn get_track(&self, id: u64) -> Result<MusicTrack, AppError> {
        self.repo
            .find_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("track {id} not found")))
    }

    /// Returns the raw file data and MIME type for streaming.
    pub async fn stream_track(&self, id: u64) -> Result<(Vec<u8>, String, u64), AppError> {
        let track = self.get_track(id).await?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(track.file_data.trim())
            .unwrap_or_else(|_| track.file_data.into_bytes());
        let size = data.len() as u64;
        Ok((data, track.mime_type, size))
    }

    pub async fn admin_create(&self, new: NewMusicTrack) -> Result<MusicTrack, AppError> {
        self.repo.save(new).await
    }

    pub async fn admin_update(
        &self,
        id: u64,
        update: MusicTrackUpdate,
    ) -> Result<MusicTrack, AppError> {
        self.repo
            .find_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("track {id} not found")))?;
        self.repo.update(id, update).await
    }

    pub async fn admin_delete(&self, id: u64) -> Result<(), AppError> {
        let _ = self.repo.delete_by_id(id).await?;
        Ok(())
    }
}
