use crate::domain::auth::password_service::PasswordService;
use crate::shared::error::AppError;

#[derive(Debug, Clone)]
pub struct BcryptPasswordHasher {
    cost: u32,
}

impl Default for BcryptPasswordHasher {
    fn default() -> Self {
        Self {
            cost: bcrypt::DEFAULT_COST,
        }
    }
}

impl PasswordService for BcryptPasswordHasher {
    fn hash(&self, raw_password: &str) -> Result<String, AppError> {
        bcrypt::hash(raw_password, self.cost)
            .map_err(|err| AppError::internal(format!("failed to hash password: {err}")))
    }

    fn verify(&self, raw_password: &str, password_hash: &str) -> Result<bool, AppError> {
        bcrypt::verify(raw_password, password_hash)
            .map_err(|err| AppError::internal(format!("failed to verify password hash: {err}")))
    }
}
