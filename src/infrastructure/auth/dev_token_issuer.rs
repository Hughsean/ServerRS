use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::auth::token_service::TokenService;
use crate::shared::error::AppError;

#[derive(Debug, Default)]
pub struct DevTokenIssuer;

impl TokenService for DevTokenIssuer {
    fn issue(&self, user_id: u64, username: &str) -> Result<String, AppError> {
        let issued_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| AppError::internal(format!("clock error: {err}")))?
            .as_secs();

        Ok(format!("dev-{user_id}-{username}-{issued_at}"))
    }

    fn verify(
        &self,
        _token: &str,
    ) -> Result<crate::domain::auth::token_service::AccessTokenClaims, AppError> {
        Err(AppError::internal(
            "DevTokenIssuer does not support verification",
        ))
    }
}
