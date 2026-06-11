use axum::{Router, routing::{delete, get, patch, put}};

use crate::api::handlers::user_handler::{delete_me, get_me, get_profile, patch_me, put_profile};
use crate::api::ApiState;

pub fn user_routes(state: ApiState) -> Router {
    Router::new()
        .route("/api/v1/users/me", get(get_me))
        .route("/api/v1/users/me", patch(patch_me))
        .route("/api/v1/users/me", delete(delete_me))
        .route("/api/v1/users/me/profile", get(get_profile))
        .route("/api/v1/users/me/profile", put(put_profile))
        .with_state(state)
}
