use crate::domain::auth::password_service::PasswordService;
use crate::shared::error::AppError;

/// Alternative, zero-cost password verifier (no hashing capability).
/// Use `BcryptPasswordHasher` for a complete `PasswordService` implementation.
#[derive(Debug, Default, Clone)]
pub struct BcryptPasswordVerifier;

impl PasswordService for BcryptPasswordVerifier {
    fn hash(&self, _raw_password: &str) -> Result<String, AppError> {
        Err(AppError::internal(
            "BcryptPasswordVerifier does not support hashing",
        ))
    }

    fn verify(&self, raw_password: &str, password_hash: &str) -> Result<bool, AppError> {
        bcrypt::verify(raw_password, password_hash)
            .map_err(|err| AppError::internal(format!("failed to verify password hash: {err}")))
    }
}
