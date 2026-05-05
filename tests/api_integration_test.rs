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
    assert!(resp["access_token"].as_str().unwrap().len() > 10);

    let token = resp["access_token"].as_str().unwrap().to_string();

    let resp = common::post(
        &app,
        "/api/v1/auth/login",
        &json!({
            "username": "testuser",
            "password": "password123!"
        }),
    )
    .await;
    assert!(resp["access_token"].as_str().unwrap().len() > 10);

    let resp = common::get_auth(&app, "/api/v1/users", &token).await;
    assert!(resp.is_array());
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
        .oneshot(Request::get("/api/v1/users").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
    let token = resp["access_token"].as_str().unwrap().to_string();
    let user_id = resp["user_id"].as_u64().unwrap();

    // Upsert profile
    let resp = common::put_auth(
        &app,
        &format!("/api/v1/users/{user_id}/profile"),
        &json!({
            "interests": ["music", "reading"],
            "personality_traits": ["calm", "thoughtful"]
        }),
        &token,
    )
    .await;
    assert_eq!(resp["user_id"].as_u64().unwrap(), user_id);

    // Get profile
    let resp = common::get_auth(&app, &format!("/api/v1/users/{user_id}"), &token).await;
    assert_eq!(resp["user_id"].as_u64().unwrap(), user_id);
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
    let token = resp["access_token"].as_str().unwrap().to_string();
    let user_id = resp["user_id"].as_u64().unwrap();

    let resp = common::put_auth(
        &app,
        &format!("/api/v1/users/{user_id}"),
        &json!({
            "nickname": "UpdatedName"
        }),
        &token,
    )
    .await;
    assert_eq!(resp["nickname"], "UpdatedName");
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
