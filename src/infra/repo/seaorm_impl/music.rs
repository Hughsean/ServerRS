use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DerivePartialModel,
    EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set, Statement, Value,
};

use crate::domain::music::{
    MusicRepoT, MusicTrack, MusicTrackListItem, MusicTrackUpdate, NewMusicTrack,
};
use crate::shared::error::AppError;

use super::super::entities::music;

fn map(m: music::Model) -> MusicTrack {
    MusicTrack {
        music_id: m.music_id,
        title: m.title,
        artist: m.artist,
        album: m.album,
        category: m.category,
        description: m.description,
        duration: m.duration,
        file_data: m.file_data,
        file_size: m.file_size,
        mime_type: m.mime_type,
        cover_image: m.cover_image,
        lyrics: m.lyrics,
        tags: m.tags,
        mood_tags: m.mood_tags,
        status: m.status,
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
    }
}

#[derive(DerivePartialModel)]
#[sea_orm(entity = "music::Entity")]
struct MusicTrackListRow {
    music_id: u64,
    title: String,
    artist: Option<String>,
    album: Option<String>,
    category: Option<String>,
    description: Option<String>,
    duration: Option<u32>,
    file_size: u64,
    mime_type: String,
    lyrics: Option<String>,
    tags: Option<sea_orm::prelude::Json>,
    mood_tags: Option<sea_orm::prelude::Json>,
    status: i8,
}

fn map_list_row(row: MusicTrackListRow) -> MusicTrackListItem {
    MusicTrackListItem {
        music_id: row.music_id,
        title: row.title,
        artist: row.artist,
        album: row.album,
        category: row.category,
        description: row.description,
        duration: row.duration,
        file_size: row.file_size,
        mime_type: row.mime_type,
        lyrics: row.lyrics,
        tags: row.tags.map(Into::into),
        mood_tags: row.mood_tags.map(Into::into),
        status: row.status,
    }
}

fn map_err(e: sea_orm::DbErr) -> AppError {
    AppError::Internal(e.to_string())
}

pub struct MusicRepo {
    db: DatabaseConnection,
}

impl MusicRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

// Note: music.file_data has `#[sea_orm(ignore)]` because SeaORM codegen cannot
// directly model LONGBLOB columns.  Inserts and updates that include file_data
// fall back to raw SQL via `execute_unprepared` while reads use the model
// (select_as = "text") path.

#[async_trait]
impl MusicRepoT for MusicRepo {
    async fn save(&self, track: NewMusicTrack) -> Result<MusicTrack, AppError> {
        let now = chrono::Utc::now();
        let stmt = Statement::from_sql_and_values(
            self.db.get_database_backend(),
            r#"
            INSERT INTO music
                (title, artist, album, category, description, duration, file_data, file_size,
                 mime_type, cover_image, lyrics, tags, mood_tags, status, created_at, updated_at)
            VALUES
                (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            vec![
                Value::String(Some(track.title)),
                Value::String(track.artist),
                Value::String(track.album),
                Value::String(track.category),
                Value::String(track.description),
                Value::Unsigned(track.duration),
                Value::Bytes(Some(track.file_data.into_bytes())),
                Value::BigUnsigned(Some(track.file_size)),
                Value::String(Some(track.mime_type)),
                Value::Bytes(track.cover_image),
                Value::String(track.lyrics),
                Value::Json(track.tags.map(Box::new)),
                Value::Json(track.mood_tags.map(Box::new)),
                Value::TinyInt(Some(1_i8)),
                Value::ChronoDateTimeUtc(Some(now)),
                Value::ChronoDateTimeUtc(Some(now)),
            ],
        );

        let result = self.db.execute_raw(stmt).await.map_err(map_err)?;
        let id = result.last_insert_id();
        let inserted = music::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| AppError::Internal("created music track not found".into()))?;
        Ok(map(inserted))
    }

    async fn find_by_id(&self, id: u64) -> Result<Option<MusicTrack>, AppError> {
        music::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)
            .map(|o| o.map(map))
    }

    async fn find_all(
        &self,
        category: Option<String>,
        search: Option<String>,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<MusicTrackListItem>, u64), AppError> {
        let mut query = music::Entity::find().filter(music::Column::Status.eq(1_i8));
        if let Some(cat) = category {
            query = query.filter(music::Column::Category.eq(cat));
        }
        if let Some(s) = search {
            let pattern = format!("%{s}%");
            query = query.filter(
                sea_orm::Condition::any()
                    .add(music::Column::Title.like(&pattern))
                    .add(music::Column::Artist.like(&pattern))
                    .add(music::Column::Album.like(&pattern)),
            );
        }
        query = query.order_by_desc(music::Column::CreatedAt);
        let paginator = query
            .into_partial_model::<MusicTrackListRow>()
            .paginate(&self.db, limit);
        let count = paginator.num_items().await.map_err(map_err)?;
        let page_num = offset / limit;
        let items = paginator.fetch_page(page_num).await.map_err(map_err)?;
        Ok((items.into_iter().map(map_list_row).collect(), count))
    }

    async fn find_all_admin(
        &self,
        category: Option<String>,
        search: Option<String>,
        status: Option<i8>,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<MusicTrackListItem>, u64), AppError> {
        let mut query = music::Entity::find();
        if let Some(category) = category {
            query = query.filter(music::Column::Category.eq(category));
        }
        if let Some(search) = search {
            let pattern = format!("%{search}%");
            query = query.filter(
                sea_orm::Condition::any()
                    .add(music::Column::Title.like(&pattern))
                    .add(music::Column::Artist.like(&pattern))
                    .add(music::Column::Album.like(&pattern)),
            );
        }
        if let Some(status) = status {
            query = query.filter(music::Column::Status.eq(status));
        }
        let paginator = query
            .order_by_desc(music::Column::CreatedAt)
            .into_partial_model::<MusicTrackListRow>()
            .paginate(&self.db, limit);
        let count = paginator.num_items().await.map_err(map_err)?;
        let items = paginator
            .fetch_page(offset / limit)
            .await
            .map_err(map_err)?;
        Ok((items.into_iter().map(map_list_row).collect(), count))
    }

    async fn update(&self, id: u64, update: MusicTrackUpdate) -> Result<MusicTrack, AppError> {
        let existing = music::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or(AppError::NotFound("track not found".into()))?;
        let mut am: music::ActiveModel = existing.into();
        if let Some(title) = update.title {
            am.title = Set(title);
        }
        if let Some(artist) = update.artist {
            am.artist = Set(artist);
        }
        if let Some(album) = update.album {
            am.album = Set(album);
        }
        if let Some(category) = update.category {
            am.category = Set(category);
        }
        if let Some(desc) = update.description {
            am.description = Set(desc);
        }
        if let Some(duration) = update.duration {
            am.duration = Set(duration);
        }
        if let Some(lyrics) = update.lyrics {
            am.lyrics = Set(lyrics);
        }
        if let Some(tags) = update.tags {
            am.tags = Set(tags);
        }
        if let Some(mood_tags) = update.mood_tags {
            am.mood_tags = Set(mood_tags);
        }
        if let Some(status) = update.status {
            am.status = Set(status);
        }
        am.updated_at = Set(chrono::Utc::now().naive_utc());
        Ok(map(am.update(&self.db).await.map_err(map_err)?))
    }

    async fn delete_by_id(&self, id: u64) -> Result<bool, AppError> {
        Ok(music::Entity::delete_by_id(id)
            .exec(&self.db)
            .await
            .map_err(map_err)?
            .rows_affected
            > 0)
    }

    async fn count_all(&self) -> Result<u64, AppError> {
        music::Entity::find().count(&self.db).await.map_err(map_err)
    }

    async fn count_trend(&self, days: u32) -> Result<Vec<(String, u64)>, AppError> {
        let since = chrono::Utc::now() - chrono::Duration::days(days as i64 - 1);
        let start = since.format("%Y-%m-%d").to_string();
        let stmt = Statement::from_sql_and_values(
            self.db.get_database_backend(),
            r#"
            SELECT DATE(created_at) AS day, COUNT(*) AS cnt
            FROM music
            WHERE created_at >= CAST(? AS DATETIME)
            GROUP BY DATE(created_at)
            ORDER BY day
            "#,
            vec![Value::String(Some(start))],
        );
        let rows = self.db.query_all_raw(stmt).await.map_err(map_err)?;
        let mut daily: Vec<(String, u64)> = rows
            .into_iter()
            .filter_map(|row| {
                let day: String = row.try_get("", "day").ok()?;
                let cnt: i64 = row.try_get("", "cnt").ok()?;
                Some((day, cnt as u64))
            })
            .collect();
        Ok(fill_trend_daily(days, &mut daily))
    }
}

fn fill_trend_daily(days: u32, daily: &mut [(String, u64)]) -> Vec<(String, u64)> {
    daily.sort_by(|a, b| a.0.cmp(&b.0));
    let mut result = Vec::with_capacity(days as usize);
    let today = chrono::Utc::now().date_naive();
    for i in (0..days).rev() {
        let date = today - chrono::Duration::days(i as i64);
        let label = date.format("%m-%d").to_string();
        let full = date.format("%Y-%m-%d").to_string();
        let count = daily
            .iter()
            .find(|(d, _)| *d == full)
            .map(|(_, c)| *c)
            .unwrap_or(0);
        result.push((label, count));
    }
    result
}
