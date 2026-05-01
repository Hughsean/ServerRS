use crate::domain::auth::password_verifier::PasswordVerifier;
use crate::shared::error::AppError;

#[derive(Debug, Default)]
pub struct PlainTextPasswordVerifier;

impl PasswordVerifier for PlainTextPasswordVerifier {
    fn verify(&self, raw_password: &str, password_hash: &str) -> Result<bool, AppError> {
        Ok(raw_password == password_hash)
    }
}
