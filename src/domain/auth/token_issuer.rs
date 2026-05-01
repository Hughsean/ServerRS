use crate::shared::error::AppError;

pub trait TokenIssuer: Send + Sync {
    fn issue(&self, user_id: u64, username: &str) -> Result<String, AppError>;
}
