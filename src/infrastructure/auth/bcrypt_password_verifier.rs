use crate::domain::auth::password_verifier::PasswordVerifier;
use crate::shared::error::AppError;

#[derive(Debug, Default, Clone)]
pub struct BcryptPasswordVerifier;

impl PasswordVerifier for BcryptPasswordVerifier {
    fn verify(&self, raw_password: &str, password_hash: &str) -> Result<bool, AppError> {
        bcrypt::verify(raw_password, password_hash)
            .map_err(|err| AppError::internal(format!("failed to verify password hash: {err}")))
    }
}
