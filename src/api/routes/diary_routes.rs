use axum::{Router, routing::get};

use crate::api::ApiState;
use crate::api::handlers::diary_handler::{
    create_diary, delete_diary, get_diary, list_diaries, update_diary,
};

pub fn diary_routes(state: ApiState) -> Router {
    Router::new()
        .route("/api/v1/diaries", get(list_diaries).post(create_diary))
        .route(
            "/api/v1/diaries/{id}",
            get(get_diary).patch(update_diary).delete(delete_diary),
        )
        .with_state(state)
}
