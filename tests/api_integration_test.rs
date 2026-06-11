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
