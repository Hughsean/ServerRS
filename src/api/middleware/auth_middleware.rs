use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, header};
use axum::middleware::Next;
use axum::response::Response;

use crate::api::ApiState;
use crate::shared::error::AppError;

pub async fn require_bearer_auth(
    State(state): State<ApiState>,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, AppError> {
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    let token = extract_bearer_token(auth_header).ok_or(AppError::Unauthorized)?;
    let auth_user = state.auth.verify(token)?;

    request.extensions_mut().insert(auth_user);
    Ok(next.run(request).await)
}

fn extract_bearer_token(auth_header: &str) -> Option<&str> {
    let mut parts = auth_header.split_whitespace();
    let schema = parts.next()?;
    let token = parts.next()?;

    if !schema.eq_ignore_ascii_case("Bearer") || token.is_empty() {
        return None;
    }

    Some(token)
}
