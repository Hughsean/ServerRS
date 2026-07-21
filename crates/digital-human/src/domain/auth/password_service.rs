use crate::shared::error::AppError;

/// 统一密码接口 — 替代 PasswordHasher + PasswordVerifier。
pub trait PasswordServiceT: Send + Sync {
    fn hash(&self, raw_password: &str) -> Result<String, AppError>;
    fn verify(&self, raw_password: &str, password_hash: &str) -> Result<bool, AppError>;
}
