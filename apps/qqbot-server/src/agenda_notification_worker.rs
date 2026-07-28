//! Agenda 到期通知扫描 Worker。
//!
//! 该 Worker 只将当前版本且已到期的事项写入既有 Owner Outbox；领取、租约与发送仍由
//! QQ Open Platform 的统一投递循环负责，避免形成第二套通知状态机。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use personal_secretary::{AgendaError, AgendaUseCase};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::AgendaConfig;
use crate::worker_lifecycle::WorkerHandle;

#[async_trait]
trait AgendaNotificationRunner: Send + Sync {
    async fn enqueue_due_notifications(&self, limit: u32) -> Result<u64, AgendaError>;
}

#[async_trait]
impl AgendaNotificationRunner for AgendaUseCase {
    async fn enqueue_due_notifications(&self, limit: u32) -> Result<u64, AgendaError> {
        AgendaUseCase::enqueue_due_notifications(self, limit).await
    }
}

pub(crate) struct AgendaNotificationHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl AgendaNotificationHandle {
    /// 发出停止信号并取出 JoinHandle，交由统一生命周期管理器回收。
    pub(crate) fn signal_and_detach(self) -> WorkerHandle {
        let _ = self.shutdown.send(true);
        WorkerHandle::new("agenda_notification", self.join)
    }
}

pub(crate) fn spawn_agenda_notification_worker(
    runner: Arc<AgendaUseCase>,
    config: AgendaConfig,
) -> AgendaNotificationHandle {
    spawn_worker(runner, config)
}

fn spawn_worker<R: AgendaNotificationRunner + 'static>(
    runner: Arc<R>,
    config: AgendaConfig,
) -> AgendaNotificationHandle {
    let (shutdown, receiver) = watch::channel(false);
    let join = tokio::spawn(run_worker(runner, config, receiver));
    AgendaNotificationHandle { shutdown, join }
}

async fn run_worker<R: AgendaNotificationRunner + 'static>(
    runner: Arc<R>,
    config: AgendaConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut consecutive_errors = 0_u32;
    loop {
        if *shutdown.borrow() {
            return;
        }
        match runner.enqueue_due_notifications(config.batch_size).await {
            Ok(enqueued) => {
                consecutive_errors = 0;
                tracing::debug!(enqueued, "agenda due-notification scan completed");
            }
            Err(error) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                tracing::warn!(error = %error, consecutive_errors, "agenda due-notification scan failed");
            }
        }
        let delay = if consecutive_errors == 0 {
            config.scan_interval_ms
        } else {
            config
                .retry_initial_ms
                .saturating_mul(2_u64.saturating_pow(consecutive_errors.saturating_sub(1)))
                .min(config.retry_max_ms)
        };
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
            }
            _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct FakeRunner(AtomicUsize);

    #[async_trait]
    impl AgendaNotificationRunner for FakeRunner {
        async fn enqueue_due_notifications(&self, _limit: u32) -> Result<u64, AgendaError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(0)
        }
    }

    #[tokio::test]
    async fn worker_runs_and_shuts_down_without_waiting_for_interval() {
        let runner = Arc::new(FakeRunner(AtomicUsize::new(0)));
        let handle = spawn_worker(
            Arc::clone(&runner),
            AgendaConfig {
                scan_interval_ms: 60_000,
                ..AgendaConfig::default()
            },
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while runner.0.load(Ordering::Relaxed) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            handle
                .signal_and_detach()
                .join_with_timeout(Duration::from_secs(1)),
        )
        .await
        .unwrap();
    }
}
