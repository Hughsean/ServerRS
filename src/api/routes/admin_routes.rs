use axum::{
    Router,
    routing::{delete, get, patch},
};

use crate::api::ApiState;
use crate::api::handlers::admin_handler::{
    delete_user, get_risk_conversation, get_user, list_risk_conversations, list_users, patch_user,
    process_risk_detection,
};

pub fn admin_routes(state: ApiState) -> Router {
    Router::new()
        .route("/api/v1/admin/users", get(list_users))
        .route("/api/v1/admin/users/{id}", get(get_user))
        .route("/api/v1/admin/users/{id}", patch(patch_user))
        .route("/api/v1/admin/users/{id}", delete(delete_user))
        .route(
            "/api/v1/admin/risk/conversations",
            get(list_risk_conversations),
        )
        .route(
            "/api/v1/admin/risk/conversations/{id}",
            get(get_risk_conversation),
        )
        .route(
            "/api/v1/admin/risk/detections/{id}/process",
            patch(process_risk_detection),
        )
        .with_state(state)
}
