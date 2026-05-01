use crate::domain::auth::password_service::PasswordService;
use crate::shared::error::AppError;

#[derive(Debug, Default)]
pub struct PlainTextPasswordHasher;

impl PasswordService for PlainTextPasswordHasher {
    fn hash(&self, raw_password: &str) -> Result<String, AppError> {
        Ok(raw_password.to_string())
    }

    fn verify(&self, raw_password: &str, password_hash: &str) -> Result<bool, AppError> {
        Ok(raw_password == password_hash)
    }
}
