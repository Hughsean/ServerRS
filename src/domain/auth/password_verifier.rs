use crate::shared::error::AppError;

pub trait PasswordVerifier: Send + Sync {
    fn verify(&self, raw_password: &str, password_hash: &str) -> Result<bool, AppError>;
}
