use axum::{
    Router,
    routing::{get, post},
};

use crate::api::ApiState;
use crate::api::handlers::psychology_handler::{
    check_favorite, get_article, get_category_tree, get_qna, get_resource, list_articles,
    list_categories, list_favorites, list_qna, list_resources, toggle_favorite,
};

pub fn psychology_routes(state: ApiState) -> Router {
    Router::new()
        .route("/api/v1/psychology/categories", get(list_categories))
        .route("/api/v1/psychology/categories/tree", get(get_category_tree))
        .route("/api/v1/psychology/articles", get(list_articles))
        .route("/api/v1/psychology/articles/{id}", get(get_article))
        .route("/api/v1/psychology/qna", get(list_qna))
        .route("/api/v1/psychology/qna/{id}", get(get_qna))
        .route("/api/v1/psychology/resources", get(list_resources))
        .route("/api/v1/psychology/resources/{id}", get(get_resource))
        .route(
            "/api/v1/psychology/favorites",
            post(toggle_favorite).get(list_favorites),
        )
        .route("/api/v1/psychology/favorites/check", get(check_favorite))
        .with_state(state)
}
