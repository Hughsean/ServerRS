//! 统一通知策略求值 Worker。
//!
//! 该 Worker 只领取已持久化的 Evaluation Request，并使用纯策略求值器完成三阶段提交；
//! 它不依赖 QQ transport，也不直接创建平台投递。

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use personal_secretary::{
    DecisionReason, EvaluationCommitResult, EvaluationPlan, NotificationOutcome,
    NotificationPolicyEvaluator, NotificationPolicyUseCase, NotificationPolicyUseCaseError,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::NotificationPolicyConfig;
use crate::worker_lifecycle::WorkerHandle;

#[async_trait]
trait NotificationPolicyRunner: Send + Sync {
    async fn recover_expired_evaluations(
        &self,
        limit: u32,
    ) -> Result<u64, NotificationPolicyUseCaseError>;

    async fn evaluate_next(
        &self,
        worker_id: &str,
        lease_secs: u64,
    ) -> Result<Option<EvaluationCommitResult>, NotificationPolicyUseCaseError>;
}

#[async_trait]
impl NotificationPolicyRunner for NotificationPolicyUseCase {
    async fn recover_expired_evaluations(
        &self,
        limit: u32,
    ) -> Result<u64, NotificationPolicyUseCaseError> {
        NotificationPolicyUseCase::recover_expired_evaluations(self, limit).await
    }

    async fn evaluate_next(
        &self,
        worker_id: &str,
        lease_secs: u64,
    ) -> Result<Option<EvaluationCommitResult>, NotificationPolicyUseCaseError> {
        NotificationPolicyUseCase::evaluate_next(self, worker_id, lease_secs, |snapshot| {
            match snapshot.evaluation_input(now_unix_secs()) {
                Ok(input) => NotificationPolicyEvaluator.evaluate(&input),
                // 解析快照中的规则歧义等错误不能让已领取 Request 永久卡住；提交为可审计终态。
                Err(_) => EvaluationPlan {
                    outcome: NotificationOutcome::EvaluationFailedTerminal,
                    reason: DecisionReason::InvalidQuietHours,
                    next_allowed_at_unix_secs: None,
                },
            }
        })
        .await
    }
}

pub(crate) struct NotificationPolicyHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl NotificationPolicyHandle {
    /// 发出停止信号并交由统一生命周期管理器以全局 deadline 回收。
    pub(crate) fn signal_and_detach(self) -> WorkerHandle {
        let _ = self.shutdown.send(true);
        WorkerHandle::new("notification_policy", self.join)
    }
}

pub(crate) fn spawn_notification_policy_worker(
    runner: Arc<NotificationPolicyUseCase>,
    config: NotificationPolicyConfig,
) -> NotificationPolicyHandle {
    spawn_worker(runner, config)
}

fn spawn_worker<R: NotificationPolicyRunner + 'static>(
    runner: Arc<R>,
    config: NotificationPolicyConfig,
) -> NotificationPolicyHandle {
    let (shutdown, receiver) = watch::channel(false);
    let join = tokio::spawn(run_worker(runner, config, receiver));
    NotificationPolicyHandle { shutdown, join }
}

async fn run_worker<R: NotificationPolicyRunner + 'static>(
    runner: Arc<R>,
    config: NotificationPolicyConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut consecutive_errors = 0_u32;
    if let Err(error) = runner
        .recover_expired_evaluations(config.recovery_limit)
        .await
    {
        consecutive_errors = 1;
        let _ = error;
        tracing::warn!(
            error_code = "evaluation_recovery_failed",
            "notification-policy expired evaluation recovery failed"
        );
    }
    loop {
        if *shutdown.borrow() {
            return;
        }
        let mut processed = 0_u32;
        let mut failed = false;
        while processed < config.batch_size {
            if *shutdown.borrow() {
                return;
            }
            match runner
                .evaluate_next(&config.worker_id, config.lease_secs)
                .await
            {
                Ok(Some(result)) => {
                    processed = processed.saturating_add(1);
                    tracing::debug!(?result, "notification-policy evaluation completed");
                }
                Ok(None) => break,
                Err(error) => {
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    failed = true;
                    let _ = error;
                    tracing::warn!(
                        error_code = "evaluation_failed",
                        consecutive_errors,
                        "notification-policy evaluation failed"
                    );
                    break;
                }
            }
        }
        if !failed {
            consecutive_errors = 0;
        }
        let delay = if processed > 0 && !failed {
            0
        } else if consecutive_errors == 0 {
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

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct FakeRunner {
        recovery_calls: AtomicUsize,
        evaluation_calls: AtomicUsize,
    }

    #[async_trait]
    impl NotificationPolicyRunner for FakeRunner {
        async fn recover_expired_evaluations(
            &self,
            _limit: u32,
        ) -> Result<u64, NotificationPolicyUseCaseError> {
            self.recovery_calls.fetch_add(1, Ordering::Relaxed);
            Ok(0)
        }

        async fn evaluate_next(
            &self,
            _worker_id: &str,
            _lease_secs: u64,
        ) -> Result<Option<EvaluationCommitResult>, NotificationPolicyUseCaseError> {
            self.evaluation_calls.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }
    }

    #[tokio::test]
    async fn worker_recovers_then_shuts_down_without_waiting_for_interval() {
        let runner = Arc::new(FakeRunner {
            recovery_calls: AtomicUsize::new(0),
            evaluation_calls: AtomicUsize::new(0),
        });
        let handle = spawn_worker(
            Arc::clone(&runner),
            NotificationPolicyConfig {
                scan_interval_ms: 60_000,
                ..NotificationPolicyConfig::default()
            },
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while runner.recovery_calls.load(Ordering::Relaxed) == 0
                || runner.evaluation_calls.load(Ordering::Relaxed) == 0
            {
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
