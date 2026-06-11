use axum::{Router, routing::get};

use crate::api::ApiState;
use crate::api::handlers::music_handler::{get_track, list_tracks, stream_track};

pub fn music_routes(state: ApiState) -> Router {
    Router::new()
        .route("/api/v1/music/tracks", get(list_tracks))
        .route("/api/v1/music/tracks/{id}", get(get_track))
        .route("/api/v1/music/tracks/{id}/stream", get(stream_track))
        .with_state(state)
}
