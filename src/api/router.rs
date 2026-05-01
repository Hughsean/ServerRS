use axum::{Router, middleware, routing::delete, routing::get, routing::post, routing::put};

use super::ApiState;
use super::handlers::auth_handler::{health, login, logout, refresh_token, register};
use super::handlers::session_handler::{
    create_session, get_session_status, list_conversation_messages, list_conversations,
    list_risk_detections, post_message,
};
use super::handlers::user_handler::{
    delete_user, get_user_profile, list_users, update_user, upsert_user_profile,
};
use super::middleware::auth_middleware::require_bearer_auth;

pub fn build_router(state: ApiState) -> Router {
    let protected = Router::new()
        .route("/api/v1/users/{user_id}", get(get_user_profile))
        .route("/api/v1/users/{user_id}", put(update_user))
        .route("/api/v1/users/{user_id}", delete(delete_user))
        .route("/api/v1/users/{user_id}/profile", put(upsert_user_profile))
        .route("/api/v1/users", get(list_users))
        .route("/api/v1/conversations/{user_id}", get(list_conversations))
        .route(
            "/api/v1/conversations/{user_id}/{conv_id}",
            get(list_conversation_messages),
        )
        .route("/api/v1/llm/sessions", post(create_session))
        .route(
            "/api/v1/llm/sessions/{session_id}/messages",
            post(post_message),
        )
        .route("/api/v1/llm/sessions/{session_id}", get(get_session_status))
        .route("/api/v1/risk-detections", get(list_risk_detections))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_auth,
        ));

    Router::new()
        .route("/health", get(health))
        .route("/api/v1/auth/register", post(register))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/refresh", post(refresh_token))
        .route("/api/v1/auth/logout", post(logout))
        .merge(protected)
        .with_state(state)
}
