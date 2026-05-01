use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::auth::token_issuer::TokenIssuer;
use crate::shared::error::AppError;

#[derive(Debug, Default)]
pub struct DevTokenIssuer;

impl TokenIssuer for DevTokenIssuer {
    fn issue(&self, user_id: u64, username: &str) -> Result<String, AppError> {
        let issued_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| AppError::internal(format!("clock error: {err}")))?
            .as_secs();

        Ok(format!("dev-{user_id}-{username}-{issued_at}"))
    }
}
