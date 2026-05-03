use async_trait::async_trait;

use super::task_event::TaskEvent;

/// Pluggable handler for TaskEvent processing.
/// Implementations can do logging, metrics, alerts, etc.
#[async_trait]
pub trait TaskHandler: Send + Sync {
    /// Process a task event.
    async fn handle(&self, event: &TaskEvent);

    /// Human-readable name for logging and diagnostics.
    fn name(&self) -> &str;
}
