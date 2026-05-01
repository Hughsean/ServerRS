use crate::shared::error::AppError;

pub trait PasswordHasher: Send + Sync {
    fn hash(&self, raw_password: &str) -> Result<String, AppError>;
}
