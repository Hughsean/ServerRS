use axum::{
    Router,
    routing::{delete, get, post, put},
};

use crate::api::ApiState;
use crate::api::handlers::community_handler::{
    create_comment, create_post, delete_comment, delete_post, get_post, like_comment, like_post,
    list_comments, list_posts, unlike_comment, unlike_post, update_post,
};

pub fn community_routes(state: ApiState) -> Router {
    Router::new()
        .route("/api/v1/community/posts", get(list_posts))
        .route("/api/v1/community/posts/{id}", get(get_post))
        .route(
            "/api/v1/community/posts/{post_id}/comments",
            get(list_comments),
        )
        .with_state(state)
}

pub fn community_protected_routes(state: ApiState) -> Router {
    Router::new()
        .route("/api/v1/community/posts", post(create_post))
        .route("/api/v1/community/posts/{id}", put(update_post))
        .route("/api/v1/community/posts/{id}", delete(delete_post))
        .route(
            "/api/v1/community/posts/{post_id}/comments",
            post(create_comment),
        )
        .route(
            "/api/v1/community/posts/{post_id}/comments/{comment_id}",
            delete(delete_comment),
        )
        // likes
        .route("/api/v1/community/posts/{post_id}/like", post(like_post))
        .route(
            "/api/v1/community/posts/{post_id}/like",
            delete(unlike_post),
        )
        .route(
            "/api/v1/community/posts/{post_id}/comments/{comment_id}/like",
            post(like_comment),
        )
        .route(
            "/api/v1/community/posts/{post_id}/comments/{comment_id}/like",
            delete(unlike_comment),
        )
        .with_state(state)
}
