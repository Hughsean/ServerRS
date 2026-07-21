use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, header};
use axum::middleware::Next;
use axum::response::Response;

use crate::api::AuthState;
use crate::api::error::ApiError as AppError;
use crate::app::auth::auth_service::{AuthenticatedUser, Role};

/// 注意：使用 State<AuthState>，而不是 State<Arc<AuthState>>。
/// 通过 `from_fn_with_state(state: AppState, require_bearer_auth)` 调用。AuthState 通过 FromRef 提取。
pub async fn require_bearer_auth(
    State(state): State<AuthState>,
    _req_method: axum::http::Method, // dummy second extractor for Axum 0.8 compat
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, AppError> {
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    let token = extract_bearer_token(auth_header).ok_or(AppError::Unauthorized)?;
    let auth_user = state.auth.authenticate(token).await?;

    request.extensions_mut().insert(auth_user);
    Ok(next.run(request).await)
}

/// 检查已认证用户是否具有 Admin 或 SuperAdmin 角色的中间件。
/// Must be placed after `require_bearer_auth` (or equivalent) so that
/// the `AuthenticatedUser` extension is present in the request.
///
/// Called via `from_fn_with_state(state: AppState, require_admin_role)`. AuthState is extracted via FromRef.
/// on a sub-router layered on top of the bearer-auth-protected routes.
pub async fn require_admin_role(
    State(_state): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, AppError> {
    let user = request
        .extensions()
        .get::<AuthenticatedUser>()
        .ok_or(AppError::Unauthorized)?;

    if !matches!(user.role, Role::Admin | Role::SuperAdmin) {
        return Err(AppError::Forbidden(format!(
            "role '{}' is not permitted for this action",
            user.role
        )));
    }

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
