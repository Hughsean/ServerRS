//! Action Planner Worker。可靠领取 + CAS + lease fencing + 指数退避。
//!
//! 复制 `thread_semantics.rs` 的 Worker 模式：`AtomicBool`+`Notify`+`max_batches_per_scan`+
//! `shutdown_changed`。每次 `run_once` 调用 `PlannerUseCase::run_once`，失败时退避重试。
//! Worker 卡死关闭、数据库错误退避和 LLM 超时都有覆盖（约束 3/8）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use personal_secretary::{PlannerRunReport, PlannerUseCase, PlannerUseCaseError};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::config::ActionPlannerConfig;
use crate::worker_lifecycle::WorkerHandle;

/// Action Planner Worker 句柄。
pub(crate) struct ActionPlannerHandle {
    shutdown: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    join: JoinHandle<()>,
}

impl ActionPlannerHandle {
    /// 发出停止信号并取出 JoinHandle，交由 [`WorkerHandle`] 统一带超时回收。
    pub(crate) fn signal_and_detach(self) -> WorkerHandle {
        self.shutdown.store(true, Ordering::Release);
        self.wake.notify_one();
        WorkerHandle::new("action_planner", self.join)
    }
}

/// Worker 运行 trait，便于测试注入假实现。
#[async_trait]
trait ActionPlannerRunner: Send + Sync {
    async fn run_once(
        &self,
        worker_id: &str,
    ) -> Result<Option<PlannerRunReport>, PlannerUseCaseError>;
}

#[async_trait]
impl ActionPlannerRunner for PlannerUseCase {
    async fn run_once(
        &self,
        worker_id: &str,
    ) -> Result<Option<PlannerRunReport>, PlannerUseCaseError> {
        PlannerUseCase::run_once(self, worker_id).await
    }
}

pub(crate) fn spawn_action_planner_worker(
    use_case: Arc<PlannerUseCase>,
    config: ActionPlannerConfig,
) -> ActionPlannerHandle {
    let shutdown = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(tokio::sync::Notify::new());
    let join = tokio::spawn(run_worker(
        use_case,
        config,
        Arc::clone(&shutdown),
        Arc::clone(&wake),
    ));
    ActionPlannerHandle {
        shutdown,
        wake,
        join,
    }
}

async fn run_worker<R: ActionPlannerRunner + 'static>(
    runner: Arc<R>,
    config: ActionPlannerConfig,
    shutdown: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
) {
    if !config.enabled {
        info!("Action Planner Worker 已禁用（action_planner.enabled=false）");
        return;
    }
    info!(
        max_batches_per_scan = config.max_batches_per_scan,
        scan_interval_ms = config.scan_interval_ms,
        lease_secs = config.lease_secs,
        "Action Planner Worker 已启动"
    );
    let mut consecutive_errors = 0u32;
    let worker_id = format!("ap-{}", std::process::id());
    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let delay = if consecutive_errors == 0 {
            config.scan_interval_ms
        } else {
            retry_delay_ms(&config, consecutive_errors)
        };
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
            _ = wake.notified() => {}
        }
        if shutdown.load(Ordering::Acquire) {
            break;
        }

        let started = Instant::now();
        let mut completed = 0usize;
        let mut suspended = 0usize;
        let mut failed = false;
        for _ in 0..config.max_batches_per_scan {
            let result = tokio::select! {
                result = runner.run_once(&worker_id) => Some(result),
                _ = shutdown_changed(&shutdown) => None,
            };
            let Some(result) = result else {
                break;
            };
            match result {
                Ok(Some(report)) => {
                    if report.completed {
                        completed += 1;
                    }
                    if report.suspended {
                        suspended += 1;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    failed = true;
                    warn!(error = %error, "Action Planner 批次失败，将退避重试");
                    break;
                }
            }
        }
        consecutive_errors = if failed {
            consecutive_errors.saturating_add(1)
        } else {
            0
        };
        if completed > 0 || suspended > 0 {
            info!(
                completed,
                suspended,
                elapsed_ms = started.elapsed().as_millis(),
                "Action Planner 扫描完成"
            );
        } else {
            tracing::trace!("Action Planner 暂无待处理 action_run");
        }
    }
    info!("Action Planner Worker 已退出");
}

fn retry_delay_ms(config: &ActionPlannerConfig, consecutive_errors: u32) -> u64 {
    let exponent = consecutive_errors.saturating_sub(1);
    let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
    config
        .retry_initial_ms
        .saturating_mul(multiplier)
        .min(config.retry_max_ms)
}

async fn shutdown_changed(shutdown: &Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_secretary::PlannerRunReport;
    use std::sync::Mutex;

    struct OkNoneRunner;

    #[async_trait]
    impl ActionPlannerRunner for OkNoneRunner {
        async fn run_once(
            &self,
            _worker_id: &str,
        ) -> Result<Option<PlannerRunReport>, PlannerUseCaseError> {
            Ok(None)
        }
    }

    struct ErrorRunner;

    #[async_trait]
    impl ActionPlannerRunner for ErrorRunner {
        async fn run_once(
            &self,
            _worker_id: &str,
        ) -> Result<Option<PlannerRunReport>, PlannerUseCaseError> {
            Err(PlannerUseCaseError::Store(
                personal_secretary::ActionStoreError::Database("test".into()),
            ))
        }
    }

    struct StuckRunner;

    #[async_trait]
    impl ActionPlannerRunner for StuckRunner {
        async fn run_once(
            &self,
            _worker_id: &str,
        ) -> Result<Option<PlannerRunReport>, PlannerUseCaseError> {
            std::future::pending().await
        }
    }

    struct CountingRunner {
        calls: Mutex<u32>,
    }

    #[async_trait]
    impl ActionPlannerRunner for CountingRunner {
        async fn run_once(
            &self,
            _worker_id: &str,
        ) -> Result<Option<PlannerRunReport>, PlannerUseCaseError> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            if *calls >= 3 {
                Ok(None)
            } else {
                Ok(Some(PlannerRunReport {
                    completed: true,
                    ..Default::default()
                }))
            }
        }
    }

    fn test_config() -> ActionPlannerConfig {
        ActionPlannerConfig {
            enabled: true,
            max_batches_per_scan: 5,
            lease_secs: 60,
            scan_interval_ms: 1,
            retry_initial_ms: 1,
            retry_max_ms: 10,
        }
    }

    #[tokio::test]
    async fn disabled_worker_exits_immediately() {
        let mut config = test_config();
        config.enabled = false;
        let shutdown = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());
        let join = tokio::spawn(run_worker(Arc::new(OkNoneRunner), config, shutdown, wake));
        tokio::time::timeout(Duration::from_millis(100), join)
            .await
            .expect("disabled worker exits immediately")
            .expect("worker does not panic");
    }

    #[tokio::test]
    async fn idle_worker_sleeps_then_exits_on_shutdown() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());
        let join = tokio::spawn(run_worker(
            Arc::new(OkNoneRunner),
            test_config(),
            Arc::clone(&shutdown),
            Arc::clone(&wake),
        ));
        tokio::time::sleep(Duration::from_millis(10)).await;
        shutdown.store(true, Ordering::Release);
        wake.notify_one();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("idle worker stops on shutdown")
            .expect("worker does not panic");
    }

    #[tokio::test]
    async fn error_worker_backs_off_and_exits_on_shutdown() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());
        let join = tokio::spawn(run_worker(
            Arc::new(ErrorRunner),
            test_config(),
            Arc::clone(&shutdown),
            Arc::clone(&wake),
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.store(true, Ordering::Release);
        wake.notify_one();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("error worker stops on shutdown")
            .expect("worker does not panic");
    }

    #[tokio::test]
    async fn shutdown_cancels_stuck_run_once() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());
        let join = tokio::spawn(run_worker(
            Arc::new(StuckRunner),
            test_config(),
            Arc::clone(&shutdown),
            Arc::clone(&wake),
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.store(true, Ordering::Release);
        wake.notify_one();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("stuck worker must stop on shutdown")
            .expect("worker must not panic");
    }

    #[tokio::test]
    async fn counting_runner_processes_until_drained() {
        let runner = Arc::new(CountingRunner {
            calls: Mutex::new(0),
        });
        let shutdown = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());
        let join = tokio::spawn(run_worker(
            Arc::clone(&runner),
            test_config(),
            Arc::clone(&shutdown),
            Arc::clone(&wake),
        ));
        // 等待处理完成
        tokio::time::sleep(Duration::from_millis(50)).await;
        let calls = *runner.calls.lock().unwrap();
        assert!(calls >= 3, "should have processed at least 3 batches");
        // 关闭
        shutdown.store(true, Ordering::Release);
        wake.notify_one();
        let _ = tokio::time::timeout(Duration::from_secs(1), join).await;
    }

    #[test]
    fn retry_delay_first_attempt_is_base() {
        let config = test_config();
        assert_eq!(retry_delay_ms(&config, 1), 1);
    }

    #[test]
    fn retry_delay_doubles() {
        let config = test_config();
        assert_eq!(retry_delay_ms(&config, 2), 2);
        assert_eq!(retry_delay_ms(&config, 3), 4);
    }

    #[test]
    fn retry_delay_capped() {
        let config = test_config();
        assert_eq!(retry_delay_ms(&config, 10), 10);
    }
}
