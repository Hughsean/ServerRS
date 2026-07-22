use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderValue, Method},
    middleware,
    routing::{delete, get, patch, post, put},
};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::TraceLayer;

use super::AppState;
use super::handlers::admin_handler::{
    delete_user as admin_delete_user, get_risk_conversation, get_user as admin_get_user,
    list_risk_conversations, list_users as admin_list_users, patch_user as admin_patch_user,
    process_risk_detection,
};
use super::handlers::auth_handler::{health, login, logout, me, refresh_token, register};
use super::handlers::chat_handler::{
    chat_disable_memory, chat_forget, chat_history, chat_memories, chat_open, chat_persona,
    chat_persona_rebuild, chat_persona_reset, chat_resume_checkpoint, chat_send_message,
    chat_transcript_clear,
};
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
use super::handlers::knowledge_review_handler::{get_review, list_reviews, publish_reviewed};
use super::handlers::music_handler::{
    admin_create_track, admin_delete_track, admin_list_tracks, admin_update_track, get_track,
    list_tracks, stream_track,
};
use super::handlers::object_handler::{
    delete_object, get_object, get_object_metadata, upload_object,
};
use super::handlers::psychology_handler::{
    admin_create_article, admin_create_category, admin_create_qna, admin_create_resource,
    admin_delete_article, admin_delete_category, admin_delete_qna, admin_delete_resource,
    admin_get_article, admin_get_category, admin_get_qna, admin_get_resource, admin_list_articles,
    admin_list_categories, admin_list_qna, admin_list_resources, admin_update_article,
    admin_update_category, admin_update_qna, admin_update_resource, check_favorite, get_article,
    get_category_tree, get_qna, get_resource, list_articles, list_categories, list_favorites,
    list_qna, list_resources, toggle_favorite, toggle_like,
};
use super::handlers::signature_handler::{create_signature, verify_signature};
use super::handlers::stats_handler::{stats_music, stats_reviews, stats_risks, stats_users};
use super::handlers::user_handler::{delete_me, get_me, get_profile, patch_me, put_profile};
use super::middleware::auth_middleware::{require_admin_role, require_bearer_auth};

pub fn build_router(state: AppState) -> Router {
    build_router_with_origins(state, &["http://localhost:3000".to_string()])
        .expect("default CORS origin must be valid")
}

pub fn build_router_with_origins(
    state: AppState,
    allowed_origins: &[String],
) -> Result<Router, String> {
    let cors = build_cors_layer(allowed_origins)?;
    let max_upload_bytes = state.object.objects.max_upload_bytes();

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
        // Chat API (new — sessionless, per-user conversation)
        .route("/api/v1/chat/open", post(chat_open))
        .route("/api/v1/chat/messages", post(chat_send_message))
        .route(
            "/api/v1/chat/checkpoints/{checkpoint_id}/resume",
            post(chat_resume_checkpoint),
        )
        .route("/api/v1/chat/history", get(chat_history))
        .route("/api/v1/chat/memories", get(chat_memories))
        .route("/api/v1/chat/persona", get(chat_persona))
        .route(
            "/api/v1/chat/memory/{id}/disable",
            post(chat_disable_memory),
        )
        .route("/api/v1/chat/persona/reset", post(chat_persona_reset))
        .route("/api/v1/chat/persona/rebuild", post(chat_persona_rebuild))
        .route("/api/v1/chat/transcript/clear", post(chat_transcript_clear))
        .route("/api/v1/chat/forget", post(chat_forget))
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
        .route(
            "/api/v1/admin/music",
            get(admin_list_tracks).post(admin_create_track),
        )
        .route("/api/v1/admin/music/{id}", patch(admin_update_track))
        .route("/api/v1/admin/music/{id}", delete(admin_delete_track))
        .route(
            "/api/v1/admin/psychology/categories",
            get(admin_list_categories).post(admin_create_category),
        )
        .route(
            "/api/v1/admin/psychology/categories/{id}",
            get(admin_get_category)
                .put(admin_update_category)
                .delete(admin_delete_category),
        )
        .route(
            "/api/v1/admin/psychology/articles",
            get(admin_list_articles).post(admin_create_article),
        )
        .route(
            "/api/v1/admin/psychology/articles/{id}",
            get(admin_get_article)
                .put(admin_update_article)
                .delete(admin_delete_article),
        )
        .route(
            "/api/v1/admin/psychology/qna",
            get(admin_list_qna).post(admin_create_qna),
        )
        .route(
            "/api/v1/admin/psychology/qna/{id}",
            get(admin_get_qna)
                .put(admin_update_qna)
                .delete(admin_delete_qna),
        )
        .route(
            "/api/v1/admin/psychology/resources",
            get(admin_list_resources).post(admin_create_resource),
        )
        .route(
            "/api/v1/admin/psychology/resources/{id}",
            get(admin_get_resource)
                .put(admin_update_resource)
                .delete(admin_delete_resource),
        )
        .route("/api/v1/admin/web-ingestion/reviews", get(list_reviews))
        .route(
            "/api/v1/admin/web-ingestion/reviews/{publish_record_id}",
            get(get_review),
        )
        .route(
            "/api/v1/admin/web-ingestion/reviews/{publish_record_id}/publish",
            post(publish_reviewed),
        )
        // ── Admin statistics ──
        .route("/api/v1/admin/stats/users", get(stats_users))
        .route("/api/v1/admin/stats/music", get(stats_music))
        .route("/api/v1/admin/stats/reviews", get(stats_reviews))
        .route("/api/v1/admin/stats/risks", get(stats_risks))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_role,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_auth,
        ));

    // ── Assemble everything into a single Router ───────────────────────────────
    let router = Router::new()
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
        // Signature — public (使用调用方提供的 appKey 做 HMAC 签名)
        .route("/api/v1/signature/create", post(create_signature))
        .route("/api/v1/signature/verify", post(verify_signature))
        // Merge protected and admin sub-routers
        .merge(protected)
        .merge(admin);

    // Global layers
    Ok(router
        .layer(DefaultBodyLimit::max(max_upload_bytes))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state))
}

fn build_cors_layer(allowed_origins: &[String]) -> Result<CorsLayer, String> {
    let origins: Vec<&str> = allowed_origins
        .iter()
        .map(String::as_str)
        .filter(|origin| !origin.trim().is_empty())
        .collect();

    let allow_origin = if origins.iter().any(|origin| *origin == "*") {
        AllowOrigin::any()
    } else {
        let parsed = origins
            .into_iter()
            .map(|origin| {
                origin
                    .parse::<HeaderValue>()
                    .map_err(|e| format!("invalid CORS origin {origin:?}: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        AllowOrigin::list(parsed)
    };

    Ok(CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any))
}
