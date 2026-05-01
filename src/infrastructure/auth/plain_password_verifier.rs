use crate::domain::auth::password_service::PasswordService;
use crate::shared::error::AppError;

/// Verifier-only — does not support hashing.
#[derive(Debug, Default)]
pub struct PlainTextPasswordVerifier;

impl PasswordService for PlainTextPasswordVerifier {
    fn hash(&self, _raw_password: &str) -> Result<String, AppError> {
        Err(AppError::internal(
            "PlainTextPasswordVerifier does not support hashing",
        ))
    }

    fn verify(&self, raw_password: &str, password_hash: &str) -> Result<bool, AppError> {
        Ok(raw_password == password_hash)
    }
}
