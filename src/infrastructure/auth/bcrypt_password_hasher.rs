use crate::domain::auth::password_hasher::PasswordHasher;
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

impl PasswordHasher for BcryptPasswordHasher {
    fn hash(&self, raw_password: &str) -> Result<String, AppError> {
        bcrypt::hash(raw_password, self.cost)
            .map_err(|err| AppError::internal(format!("failed to hash password: {err}")))
    }
}
