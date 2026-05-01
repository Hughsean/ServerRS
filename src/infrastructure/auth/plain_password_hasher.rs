use crate::domain::auth::password_hasher::PasswordHasher;
use crate::shared::error::AppError;

#[derive(Debug, Default)]
pub struct PlainTextPasswordHasher;

impl PasswordHasher for PlainTextPasswordHasher {
    fn hash(&self, raw_password: &str) -> Result<String, AppError> {
        Ok(raw_password.to_string())
    }
}
