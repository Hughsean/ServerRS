//! Agenda 到期通知扫描 Worker。
//!
//! 该 Worker 只将当前版本且已到期的事项生成 Notification Candidate 与 Evaluation Request；
//! 策略求值与 QQ Outbox 投递由后续独立 Worker 负责，禁止来源扫描绕过统一策略。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use personal_secretary::{AgendaError, AgendaUseCase, NotificationCandidateProductionReport};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::AgendaConfig;
use crate::worker_lifecycle::WorkerHandle;

#[async_trait]
trait AgendaNotificationRunner: Send + Sync {
    async fn produce_due_notification_candidates(
        &self,
        limit: u32,
    ) -> Result<NotificationCandidateProductionReport, AgendaError>;
}

#[async_trait]
impl AgendaNotificationRunner for AgendaUseCase {
    async fn produce_due_notification_candidates(
        &self,
        limit: u32,
    ) -> Result<NotificationCandidateProductionReport, AgendaError> {
        AgendaUseCase::produce_due_notification_candidates(self, limit).await
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
        match runner
            .produce_due_notification_candidates(config.batch_size)
            .await
        {
            Ok(report) => {
                consecutive_errors = 0;
                tracing::debug!(
                    candidates_created = report.candidates_created,
                    requests_created = report.requests_created,
                    sources_skipped_stale = report.sources_skipped_stale,
                    "agenda notification-candidate scan completed"
                );
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
        async fn produce_due_notification_candidates(
            &self,
            _limit: u32,
        ) -> Result<NotificationCandidateProductionReport, AgendaError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(NotificationCandidateProductionReport::default())
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
