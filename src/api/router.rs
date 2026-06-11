use axum::{
    Router, middleware,
    routing::{delete, get, patch, post, put},
};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use super::ApiState;
use super::handlers::admin_handler::{
    delete_user as admin_delete_user, get_risk_conversation, get_user as admin_get_user,
    list_risk_conversations, list_users as admin_list_users, patch_user as admin_patch_user,
    process_risk_detection,
};
use super::handlers::auth_handler::{health, login, logout, me, refresh_token, register};
use super::handlers::community_handler::{
    create_comment, create_post, delete_comment, delete_post, get_post, like_comment, like_post,
    list_comments, list_posts, unlike_comment, unlike_post, update_post,
};
use super::handlers::depression_handler::{
    create_assessment, delete_assessment, get_assessment, get_scale, list_assessments, list_scales,
};
use super::handlers::diary_handler::{
    create_diary, delete_diary, get_diary, list_diaries, update_diary,
};
use super::handlers::music_handler::{
    admin_create_track, admin_delete_track, admin_update_track, get_track, list_tracks,
    stream_track,
};
use super::handlers::object_handler::{
    delete_object, get_object, get_object_metadata, upload_object,
};
use super::handlers::psychology_handler::{
    check_favorite, get_article, get_category_tree, get_qna, get_resource, list_articles,
    list_categories, list_favorites, list_qna, list_resources, toggle_favorite, toggle_like,
};
use super::handlers::session_handler::{
    create_session, get_session_status, list_conversation_messages, list_conversations,
    list_risk_detections, post_message,
};
use super::handlers::user_handler::{delete_me, get_me, get_profile, patch_me, put_profile};
use super::middleware::auth_middleware::{require_admin_role, require_bearer_auth};

pub fn build_router(state: ApiState) -> Router {
    // ── Protected routes (require valid Bearer token) ──────────────────────────
    let protected = Router::new()
        // Auth
        .route("/api/v1/auth/me", get(me))
        // Users / Me
        .route("/api/v1/users/me", get(get_me))
        .route("/api/v1/users/me", patch(patch_me))
        .route("/api/v1/users/me", delete(delete_me))
        .route("/api/v1/users/me/profile", get(get_profile))
        .route("/api/v1/users/me/profile", put(put_profile))
        // Conversations
        .route(
            "/api/v1/users/{user_id}/conversations",
            get(list_conversations),
        )
        .route(
            "/api/v1/users/{user_id}/conversations/{conv_id}",
            get(list_conversation_messages),
        )
        // LLM Sessions
        .route("/api/v1/llm/sessions", post(create_session))
        .route(
            "/api/v1/llm/sessions/{session_id}/messages",
            post(post_message),
        )
        .route("/api/v1/llm/sessions/{session_id}", get(get_session_status))
        // Risk detections
        .route("/api/v1/risk-detections", get(list_risk_detections))
        // Depression scales (read) + assessments (write)
        .route("/api/v1/depression/assessments", get(list_assessments))
        .route("/api/v1/depression/assessments", post(create_assessment))
        .route(
            "/api/v1/depression/assessments/{assessment_id}",
            get(get_assessment),
        )
        .route(
            "/api/v1/depression/assessments/{assessment_id}",
            delete(delete_assessment),
        )
        // Diary CRUD
        .route("/api/v1/diaries", get(list_diaries))
        .route("/api/v1/diaries", post(create_diary))
        .route("/api/v1/diaries/{id}", get(get_diary))
        .route("/api/v1/diaries/{id}", patch(update_diary))
        .route("/api/v1/diaries/{id}", delete(delete_diary))
        // Psychology favorites
        .route("/api/v1/psychology/favorites", get(list_favorites))
        .route("/api/v1/psychology/favorites", post(toggle_favorite))
        .route("/api/v1/psychology/favorites/check", get(check_favorite))
        .route("/api/v1/psychology/likes", post(toggle_like))
        // Community posts (write)
        .route("/api/v1/community/posts", post(create_post))
        .route("/api/v1/community/posts/{id}", put(update_post))
        .route("/api/v1/community/posts/{id}", delete(delete_post))
        // Community comments (write)
        .route(
            "/api/v1/community/posts/{post_id}/comments",
            post(create_comment),
        )
        .route(
            "/api/v1/community/posts/{post_id}/comments/{comment_id}",
            delete(delete_comment),
        )
        // Community likes
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
        // Object storage
        .route("/api/v1/objects/upload", post(upload_object))
        .route("/api/v1/objects/{object_id}", get(get_object))
        .route(
            "/api/v1/objects/{object_id}/metadata",
            get(get_object_metadata),
        )
        .route("/api/v1/objects/{object_id}", delete(delete_object))
        // Apply bearer-auth middleware to all protected routes
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_auth,
        ));

    // ── Admin routes (require admin role on top of bearer auth) ────────────────
    let admin = Router::new()
        .route("/api/v1/admin/users", get(admin_list_users))
        .route("/api/v1/admin/users/{id}", get(admin_get_user))
        .route("/api/v1/admin/users/{id}", patch(admin_patch_user))
        .route("/api/v1/admin/users/{id}", delete(admin_delete_user))
        .route(
            "/api/v1/admin/risk-conversations",
            get(list_risk_conversations),
        )
        .route(
            "/api/v1/admin/risk-conversations/{id}",
            get(get_risk_conversation),
        )
        .route(
            "/api/v1/admin/risk-detections/{id}/process",
            post(process_risk_detection),
        )
        .route("/api/v1/admin/music", post(admin_create_track))
        .route("/api/v1/admin/music/{id}", patch(admin_update_track))
        .route("/api/v1/admin/music/{id}", delete(admin_delete_track))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_auth,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_role,
        ));

    // ── Assemble everything into a single Router ───────────────────────────────
    Router::new()
        // Health
        .route("/health", get(health))
        // Auth public endpoints
        .route("/api/v1/auth/register", post(register))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/refresh", post(refresh_token))
        .route("/api/v1/auth/logout", post(logout))
        // Psychology — public read endpoints
        .route("/api/v1/psychology/categories", get(list_categories))
        .route("/api/v1/psychology/categories/tree", get(get_category_tree))
        .route("/api/v1/psychology/articles", get(list_articles))
        .route("/api/v1/psychology/articles/{id}", get(get_article))
        .route("/api/v1/psychology/qna", get(list_qna))
        .route("/api/v1/psychology/qna/{id}", get(get_qna))
        .route("/api/v1/psychology/resources", get(list_resources))
        .route("/api/v1/psychology/resources/{id}", get(get_resource))
        // Music — public read endpoints
        .route("/api/v1/music/tracks", get(list_tracks))
        .route("/api/v1/music/tracks/{id}", get(get_track))
        .route("/api/v1/music/tracks/{id}/stream", get(stream_track))
        // Depression scales — public read
        .route("/api/v1/depression/scales", get(list_scales))
        .route("/api/v1/depression/scales/{scale_id}", get(get_scale))
        // Community — public read
        .route("/api/v1/community/posts", get(list_posts))
        .route("/api/v1/community/posts/{id}", get(get_post))
        .route(
            "/api/v1/community/posts/{post_id}/comments",
            get(list_comments),
        )
        // Merge protected and admin sub-routers
        .merge(protected)
        .merge(admin)
        // Global layers
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
