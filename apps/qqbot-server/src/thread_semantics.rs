use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use personal_secretary::{ThreadSemanticRun, ThreadSemanticUseCase, ThreadSemanticUseCaseError};
use tokio::task::JoinHandle;

use crate::config::ThreadSemanticsConfig;

#[async_trait]
trait ThreadSemanticRunner: Send + Sync {
    async fn run_once(&self) -> Result<Option<ThreadSemanticRun>, ThreadSemanticUseCaseError>;
}

#[async_trait]
impl ThreadSemanticRunner for ThreadSemanticUseCase {
    async fn run_once(&self) -> Result<Option<ThreadSemanticRun>, ThreadSemanticUseCaseError> {
        ThreadSemanticUseCase::run_once(self).await
    }
}

pub(crate) struct ThreadSemanticsHandle {
    shutdown: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    join: JoinHandle<()>,
}

impl ThreadSemanticsHandle {
    pub(crate) async fn shutdown(self) {
        self.shutdown.store(true, Ordering::Release);
        self.wake.notify_one();
        let _ = self.join.await;
    }
}

pub(crate) fn spawn_thread_semantics_worker(
    use_case: Arc<ThreadSemanticUseCase>,
    config: ThreadSemanticsConfig,
) -> ThreadSemanticsHandle {
    let shutdown = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(tokio::sync::Notify::new());
    let join = tokio::spawn(run_worker(
        use_case,
        config,
        Arc::clone(&shutdown),
        Arc::clone(&wake),
    ));
    ThreadSemanticsHandle {
        shutdown,
        wake,
        join,
    }
}

async fn run_worker<R: ThreadSemanticRunner + 'static>(
    runner: Arc<R>,
    config: ThreadSemanticsConfig,
    shutdown: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
) {
    if !config.enabled {
        tracing::info!("线程类型化语义 Worker 已禁用（thread_semantics.enabled=false）");
        return;
    }
    tracing::info!(
        max_events = config.max_events,
        max_total_chars = config.max_total_chars,
        max_batches_per_scan = config.max_batches_per_scan,
        scan_interval_ms = config.scan_interval_ms,
        "线程类型化语义 Worker 已启动；仅生成可追溯 proposed 候选"
    );
    let mut consecutive_errors = 0u32;
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
        let mut events = 0usize;
        let mut claims = 0usize;
        let mut decisions = 0usize;
        let mut questions = 0usize;
        let mut failed = false;
        for _ in 0..config.max_batches_per_scan {
            let result = tokio::select! {
                result = runner.run_once() => Some(result),
                _ = shutdown_changed(&shutdown) => None,
            };
            let Some(result) = result else {
                break;
            };
            match result {
                Ok(Some(run)) => {
                    events += run.events_read;
                    claims += run.claims_created;
                    decisions += run.decisions_created;
                    questions += run.questions_created;
                }
                Ok(None) => break,
                Err(error) => {
                    failed = true;
                    tracing::warn!(error = %error, "线程类型化语义批次失败，将退避重试");
                    break;
                }
            }
        }
        consecutive_errors = if failed {
            consecutive_errors.saturating_add(1)
        } else {
            0
        };
        if events > 0 {
            tracing::debug!(
                events_read = events,
                claims_created = claims,
                decisions_created = decisions,
                questions_created = questions,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "线程类型化语义扫描完成"
            );
        } else {
            tracing::trace!("线程类型化语义暂无待处理事件");
        }
    }
    tracing::info!("线程类型化语义 Worker 已退出");
}

fn retry_delay_ms(config: &ThreadSemanticsConfig, consecutive_errors: u32) -> u64 {
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

    struct StuckRunner;

    #[async_trait]
    impl ThreadSemanticRunner for StuckRunner {
        async fn run_once(&self) -> Result<Option<ThreadSemanticRun>, ThreadSemanticUseCaseError> {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    #[test]
    fn retry_is_exponential_and_capped() {
        let config = ThreadSemanticsConfig {
            retry_initial_ms: 100,
            retry_max_ms: 500,
            ..ThreadSemanticsConfig::default()
        };
        assert_eq!(retry_delay_ms(&config, 1), 100);
        assert_eq!(retry_delay_ms(&config, 2), 200);
        assert_eq!(retry_delay_ms(&config, 4), 500);
    }

    #[tokio::test]
    async fn shutdown_cancels_stuck_semantic_extraction() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());
        let join = tokio::spawn(run_worker(
            Arc::new(StuckRunner),
            ThreadSemanticsConfig {
                scan_interval_ms: 1,
                ..ThreadSemanticsConfig::default()
            },
            Arc::clone(&shutdown),
            Arc::clone(&wake),
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.store(true, Ordering::Release);
        wake.notify_one();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("semantic worker must stop while extraction is stuck")
            .expect("semantic worker must not panic");
    }
}
