use async_trait::async_trait;

use super::task_event::TaskEvent;

/// TaskEvent 处理的可插拔处理器。
/// 实现可以执行日志记录、指标收集、告警等。
#[async_trait]
pub trait TaskHandler: Send + Sync {
    /// 处理任务事件。
    async fn handle(&self, event: &TaskEvent);

    /// 用于日志和诊断的人类可读名称。
    fn name(&self) -> &str;
}
