use std::time::Instant;

use axum::{extract::Request, middleware::Next, response::Response};

use crate::api::error::ApiErrorLogContext;

/// Logs one actionable event for every HTTP 5xx response. The default
/// `tower_http::TraceLayer` failure event only contains status and latency;
/// this middleware adds the request method/URI and the original `ApiError`.
pub async fn log_http_failures(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    // Log only the path. Query strings can contain signed URLs, search terms,
    // or other values that should not be copied into application logs.
    let path = request.uri().path().to_owned();
    let started_at = Instant::now();
    let response = next.run(request).await;
    let status = response.status();

    if status.is_server_error() {
        let latency_ms = started_at.elapsed().as_secs_f64() * 1_000.0;
        if let Some(error) = response.extensions().get::<ApiErrorLogContext>() {
            tracing::error!(
                http.method = %method,
                http.path = %path,
                http.status = status.as_u16(),
                latency_ms,
                error.code = error.code,
                error.message = %error.message,
                "HTTP 请求失败"
            );
        } else {
            tracing::error!(
                http.method = %method,
                http.path = %path,
                http.status = status.as_u16(),
                latency_ms,
                "HTTP 请求失败（响应未携带 ApiError 上下文）"
            );
        }
    }

    response
}
