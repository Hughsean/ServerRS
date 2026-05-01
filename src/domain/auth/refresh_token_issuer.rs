use crate::shared::error::AppError;

pub trait RefreshTokenIssuer: Send + Sync {
    fn issue_refresh(&self, user_id: u64, username: &str) -> Result<String, AppError>;
}
