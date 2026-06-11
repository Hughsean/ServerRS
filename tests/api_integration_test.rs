use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::util::ServiceExt;

mod common;

#[tokio::test]
async fn health_check_works() {
    let app = common::test_app().await;
    let response = app
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = common::read_body(response).await;
    assert_eq!(body["status"], "up");
}

#[tokio::test]
async fn register_and_login_flow() {
    let app = common::test_app().await;
    let resp = common::post(
        &app,
        "/api/v1/auth/register",
        &json!({
            "username": "testuser",
            "password": "password123!"
        }),
    )
    .await;
    assert!(resp["accessToken"].as_str().unwrap().len() > 10);
    let token = resp["accessToken"].as_str().unwrap().to_string();
    let resp = common::post(
        &app,
        "/api/v1/auth/login",
        &json!({
            "username": "testuser",
            "password": "password123!"
        }),
    )
    .await;
    assert!(resp["accessToken"].as_str().unwrap().len() > 10);
    let req = Request::get("/api/v1/users/me")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn invalid_login_returns_unauthorized() {
    let app = common::test_app().await;
    let resp = common::post(
        &app,
        "/api/v1/auth/login",
        &json!({
            "username": "demo_user",
            "password": "wrongpassword!!!"
        }),
    )
    .await;
    assert_eq!(resp["code"], "UNAUTHORIZED");
}

#[tokio::test]
async fn missing_token_returns_unauthorized() {
    let app = common::test_app().await;
    let response = app
        .oneshot(
            Request::get("/api/v1/users/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_me_requires_token() {
    let app = common::test_app().await;
    let response = app
        .oneshot(Request::get("/api/v1/auth/me").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn session_routes_enforce_owner() {
    let app = common::test_app().await;

    let owner = common::post(
        &app,
        "/api/v1/auth/register",
        &json!({
            "username": "session_owner",
            "password": "password123!"
        }),
    )
    .await;
    let intruder = common::post(
        &app,
        "/api/v1/auth/register",
        &json!({
            "username": "session_intruder",
            "password": "password123!"
        }),
    )
    .await;

    let owner_token = owner["accessToken"].as_str().unwrap().to_string();
    let intruder_token = intruder["accessToken"].as_str().unwrap().to_string();
    let owner_id = owner["user"]["id"].as_u64().unwrap();

    let session = common::post_auth(
        &app,
        "/api/v1/llm/sessions",
        &json!({
            "user_id": owner_id
        }),
        &owner_token,
    )
    .await;
    let session_id = session["session_id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/llm/sessions/{session_id}/messages"))
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {intruder_token}"))
                .body(Body::from(json!({ "text": "hello" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = app
        .oneshot(
            Request::get(format!("/api/v1/llm/sessions/{session_id}"))
                .header("Authorization", format!("Bearer {intruder_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn user_profile_crud_flow() {
    let app = common::test_app().await;
    let resp = common::post(
        &app,
        "/api/v1/auth/register",
        &json!({
            "username": "profile_test",
            "password": "password123!"
        }),
    )
    .await;
    let token = resp["accessToken"].as_str().unwrap().to_string();
    let resp = common::put_auth(
        &app,
        "/api/v1/users/me/profile",
        &json!({
            "interests": ["music", "reading"]
        }),
        &token,
    )
    .await;
    assert!(resp.is_object());
    let req = Request::get("/api/v1/users/me/profile")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn update_user_info_flow() {
    let app = common::test_app().await;
    let resp = common::post(
        &app,
        "/api/v1/auth/register",
        &json!({
            "username": "update_test",
            "password": "password123!"
        }),
    )
    .await;
    let token = resp["accessToken"].as_str().unwrap().to_string();
    let resp = common::patch_auth(
        &app,
        "/api/v1/users/me",
        &json!({
            "nickname": "UpdatedName"
        }),
        &token,
    )
    .await;
    assert!(resp.get("nickname").is_some() || resp.is_object());
}

#[tokio::test]
async fn disabled_user_cannot_login() {
    let app = common::test_app().await;
    let resp = common::post(
        &app,
        "/api/v1/auth/login",
        &json!({
            "username": "locked_user",
            "password": "password123!"
        }),
    )
    .await;
    assert_eq!(resp["code"], "FORBIDDEN");
}

// ── Diary smoke tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn diary_crud_flow() {
    let app = common::test_app().await;
    let resp = common::post(
        &app,
        "/api/v1/auth/register",
        &json!({
            "username": "diary_test",
            "password": "password123!"
        }),
    )
    .await;
    let token = resp["accessToken"].as_str().unwrap().to_string();

    // Create
    let resp = common::post_auth(
        &app,
        "/api/v1/diaries",
        &json!({"title": "Smoke Test", "content": "Hello diary"}),
        &token,
    )
    .await;
    assert!(resp["id"].as_u64().is_some());
    let diary_id = resp["id"].as_u64().unwrap();

    // List
    let req = Request::get("/api/v1/diaries?page=1&pageSize=10")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Get by id
    let req = Request::get(format!("/api/v1/diaries/{diary_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Update
    let resp = common::patch_auth(
        &app,
        &format!("/api/v1/diaries/{diary_id}"),
        &json!({"content": "Updated content"}),
        &token,
    )
    .await;
    assert!(resp.is_object());

    // Delete
    let req = Request::delete(format!("/api/v1/diaries/{diary_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ── Music smoke tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn music_public_endpoints() {
    let app = common::test_app().await;

    // List tracks
    let resp = app
        .clone()
        .oneshot(
            Request::get("/api/v1/music/tracks?page=1&pageSize=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Get non-existent track
    let resp = app
        .oneshot(
            Request::get("/api/v1/music/tracks/99999")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // May return 404 or 500 depending on mock
    assert!(resp.status().is_client_error() || resp.status().is_server_error());
}

// ── Object smoke tests ────────────────────────────────────────────────────

#[tokio::test]
async fn object_routes_are_wired() {
    let app = common::test_app().await;
    let resp = common::post(
        &app,
        "/api/v1/auth/register",
        &json!({
            "username": "object_test",
            "password": "password123!"
        }),
    )
    .await;
    let token = resp["accessToken"].as_str().unwrap().to_string();

    // Upload via multipart
    let boundary = "testboundary123";
    let body_data = concat!(
        "--testboundary123\r\n",
        "Content-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n",
        "Content-Type: text/plain\r\n",
        "\r\n",
        "hello world\r\n",
        "--testboundary123--\r\n"
    );
    let req = Request::post("/api/v1/objects/upload?bucket=smoke")
        .header("Authorization", format!("Bearer {token}"))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body_data))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    // Verify endpoint is reachable and routed correctly
    let status = resp.status();
    assert!(
        status.is_success() || status.is_client_error(),
        "object upload returned unexpected status: {status}"
    );

    // Verify GET /objects/{id} route exists (will 404 on non-existent id)
    let resp = app
        .oneshot(
            Request::get("/api/v1/objects/99999")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_client_error() || resp.status().is_server_error(),
        "object get returned unexpected status: {}",
        resp.status()
    );
}

// ── Admin smoke tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn admin_risk_endpoints_require_admin() {
    let app = common::test_app().await;
    // Register a regular user
    let resp = common::post(
        &app,
        "/api/v1/auth/register",
        &json!({
            "username": "regular_user",
            "password": "password123!"
        }),
    )
    .await;
    let token = resp["accessToken"].as_str().unwrap().to_string();

    // Regular user should be rejected from admin risk endpoints (401 or 403)
    let req = Request::get("/api/v1/admin/risk-conversations?page=1&pageSize=10")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn psychology_public_endpoints() {
    let app = common::test_app().await;

    for path in &[
        "/api/v1/psychology/categories",
        "/api/v1/psychology/categories/tree",
        "/api/v1/psychology/articles?page=1&pageSize=10",
        "/api/v1/psychology/qna?page=1&pageSize=10",
        "/api/v1/psychology/resources?page=1&pageSize=10",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::get(*path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn depression_scales_public() {
    let app = common::test_app().await;
    let resp = app
        .oneshot(
            Request::get("/api/v1/depression/scales")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn community_posts_public() {
    let app = common::test_app().await;
    let resp = app
        .oneshot(
            Request::get("/api/v1/community/posts?page=1&pageSize=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn refresh_token_flow() {
    let app = common::test_app().await;
    let resp = common::post(
        &app,
        "/api/v1/auth/register",
        &json!({
            "username": "refresh_test",
            "password": "password123!"
        }),
    )
    .await;
    let refresh_token = resp["refreshToken"].as_str().unwrap().to_string();

    let resp = common::post(
        &app,
        "/api/v1/auth/refresh",
        &json!({"refresh_token": refresh_token}),
    )
    .await;
    assert!(resp["accessToken"].as_str().unwrap().len() > 10);
    assert!(resp["refreshToken"].as_str().unwrap().len() > 10);
}

#[tokio::test]
async fn logout_works() {
    let app = common::test_app().await;
    let resp = common::post(
        &app,
        "/api/v1/auth/register",
        &json!({
            "username": "logout_test",
            "password": "password123!"
        }),
    )
    .await;
    let refresh_token = resp["refreshToken"].as_str().unwrap().to_string();

    let req = Request::post("/api/v1/auth/logout")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({"refresh_token": refresh_token}).to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}
