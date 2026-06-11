use axum::{Router, routing::get};

use crate::api::ApiState;
use crate::api::handlers::depression_handler::{
    create_assessment, delete_assessment, get_assessment, get_scale, list_assessments, list_scales,
};

pub fn depression_routes(state: ApiState) -> Router {
    Router::new()
        .route("/api/v1/depression/scales", get(list_scales))
        .route("/api/v1/depression/scales/{id}", get(get_scale))
        .route(
            "/api/v1/depression/assessments",
            get(list_assessments).post(create_assessment),
        )
        .route(
            "/api/v1/depression/assessments/{id}",
            get(get_assessment).delete(delete_assessment),
        )
        .with_state(state)
}
