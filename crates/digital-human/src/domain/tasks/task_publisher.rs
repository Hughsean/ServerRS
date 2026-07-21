use async_trait::async_trait;

use crate::domain::tasks::task_event::TaskEvent;
use crate::shared::error::AppError;

#[async_trait]
pub trait TaskPublisher: Send + Sync {
    async fn publish(&self, event: TaskEvent) -> Result<(), AppError>;
}
