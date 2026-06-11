use axum::{
    Router,
    routing::{get, post},
};

use crate::api::ApiState;
use crate::api::handlers::auth_handler::{health, login, logout, me, refresh_token, register};

pub fn auth_routes(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/auth/register", post(register))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/refresh", post(refresh_token))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
        .with_state(state)
}
