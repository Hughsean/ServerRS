use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use personal_secretary::{FollowUpScanReport, FollowUpUseCase, InboundEventStoreError};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::FollowUpConfig;
use crate::worker_lifecycle::WorkerHandle;

#[async_trait]
trait FollowUpRunner: Send + Sync {
    async fn scan(
        &self,
        now_unix_secs: i64,
        horizon_secs: i64,
        response_timeout_secs: i64,
        blocker_escalation_secs: i64,
        limit: u32,
    ) -> Result<FollowUpScanReport, InboundEventStoreError>;
}

#[async_trait]
impl FollowUpRunner for FollowUpUseCase {
    async fn scan(
        &self,
        now_unix_secs: i64,
        horizon_secs: i64,
        response_timeout_secs: i64,
        blocker_escalation_secs: i64,
        limit: u32,
    ) -> Result<FollowUpScanReport, InboundEventStoreError> {
        FollowUpUseCase::scan(
            self,
            now_unix_secs,
            horizon_secs,
            response_timeout_secs,
            blocker_escalation_secs,
            limit,
        )
        .await
    }
}

pub(crate) struct FollowUpHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl FollowUpHandle {
    /// 发出停止信号并取出 JoinHandle，交由 [`WorkerHandle`] 统一带超时回收。
    pub(crate) fn signal_and_detach(self) -> WorkerHandle {
        let _ = self.shutdown.send(true);
        WorkerHandle::new("follow_up", self.join)
    }
}

pub(crate) fn spawn_follow_up_worker(
    runner: Arc<FollowUpUseCase>,
    config: FollowUpConfig,
) -> FollowUpHandle {
    spawn_worker(runner, config)
}

fn spawn_worker<R: FollowUpRunner + 'static>(
    runner: Arc<R>,
    config: FollowUpConfig,
) -> FollowUpHandle {
    let (shutdown, receiver) = watch::channel(false);
    let join = tokio::spawn(run_worker(runner, config, receiver));
    FollowUpHandle { shutdown, join }
}

async fn run_worker<R: FollowUpRunner + 'static>(
    runner: Arc<R>,
    config: FollowUpConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut consecutive_errors = 0_u32;
    loop {
        if *shutdown.borrow() {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .min(i64::MAX as u64) as i64;
        match runner
            .scan(
                now,
                config.horizon_secs,
                config.response_timeout_secs,
                config.blocker_escalation_secs,
                config.batch_size,
            )
            .await
        {
            Ok(report) => {
                consecutive_errors = 0;
                tracing::debug!(
                    commitments_materialized = report.commitments_materialized,
                    items_reconciled = report.items_reconciled,
                    notification_candidates_created = report.notification_candidates_created,
                    notification_evaluation_requests_created =
                        report.notification_evaluation_requests_created,
                    memories_expired = report.memories_expired,
                    response_expectations_materialized = report.response_expectations_materialized,
                    response_expectations_resolved = report.response_expectations_resolved,
                    project_blockers_materialized = report.project_blockers_materialized,
                    "follow-up maintenance scan completed"
                );
            }
            Err(error) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                tracing::warn!(error = %error, consecutive_errors, "follow-up maintenance scan failed");
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
    impl FollowUpRunner for FakeRunner {
        async fn scan(
            &self,
            _now_unix_secs: i64,
            _horizon_secs: i64,
            _response_timeout_secs: i64,
            _blocker_escalation_secs: i64,
            _limit: u32,
        ) -> Result<FollowUpScanReport, InboundEventStoreError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(FollowUpScanReport::default())
        }
    }

    #[tokio::test]
    async fn worker_runs_and_shuts_down_without_waiting_for_interval() {
        let runner = Arc::new(FakeRunner(AtomicUsize::new(0)));
        let handle = spawn_worker(
            Arc::clone(&runner),
            FollowUpConfig {
                scan_interval_ms: 60_000,
                ..FollowUpConfig::default()
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
