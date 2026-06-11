use axum::{Router, middleware, routing::delete, routing::get, routing::post};

use crate::api::ApiState;
use crate::api::handlers::object_handler::{
    delete_object, get_object, get_object_metadata, upload_object,
};
use crate::api::middleware::auth_middleware::require_bearer_auth;

pub fn object_routes(state: ApiState) -> Router {
    let protected = Router::new()
        .route("/api/v1/objects", post(upload_object))
        .route("/api/v1/objects/{objectId}", delete(delete_object))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_auth,
        ));

    Router::new()
        .route("/api/v1/objects/{objectId}", get(get_object))
        .route("/api/v1/objects/{objectId}/metadata", get(get_object_metadata))
        .merge(protected)
        .with_state(state)
}
