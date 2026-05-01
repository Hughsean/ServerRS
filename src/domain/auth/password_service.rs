use crate::shared::error::AppError;

/// Unified password trait — replaces PasswordHasher + PasswordVerifier.
pub trait PasswordService: Send + Sync {
    fn hash(&self, raw_password: &str) -> Result<String, AppError>;
    fn verify(&self, raw_password: &str, password_hash: &str) -> Result<bool, AppError>;
}
