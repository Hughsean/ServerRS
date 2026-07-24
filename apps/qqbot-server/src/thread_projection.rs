use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use personal_secretary::{ThreadProjectionError, ThreadProjectionRun, ThreadProjectionUseCase};
use tokio::task::JoinHandle;

use crate::config::ThreadProjectionConfig;

#[async_trait]
trait ThreadProjectionRunner: Send + Sync {
    async fn run_once(&self) -> Result<Option<ThreadProjectionRun>, ThreadProjectionError>;
}

#[async_trait]
impl ThreadProjectionRunner for ThreadProjectionUseCase {
    async fn run_once(&self) -> Result<Option<ThreadProjectionRun>, ThreadProjectionError> {
        ThreadProjectionUseCase::run_once(self).await
    }
}

pub(crate) struct ThreadProjectionHandle {
    shutdown: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    join: JoinHandle<()>,
}

impl ThreadProjectionHandle {
    pub(crate) async fn shutdown(self) {
        self.shutdown.store(true, Ordering::Release);
        self.wake.notify_one();
        let _ = self.join.await;
    }
}

pub(crate) fn spawn_thread_projection_worker(
    use_case: Arc<ThreadProjectionUseCase>,
    config: ThreadProjectionConfig,
) -> ThreadProjectionHandle {
    let shutdown = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(tokio::sync::Notify::new());
    let join = tokio::spawn(run_worker(
        use_case,
        config,
        Arc::clone(&shutdown),
        Arc::clone(&wake),
    ));
    ThreadProjectionHandle {
        shutdown,
        wake,
        join,
    }
}

async fn run_worker<R: ThreadProjectionRunner + 'static>(
    runner: Arc<R>,
    config: ThreadProjectionConfig,
    shutdown: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
) {
    if !config.enabled {
        tracing::info!("确定性线程投影 Worker 已禁用（thread_projection.enabled=false）");
        return;
    }
    tracing::info!(
        batch_size = config.batch_size,
        max_batches_per_scan = config.max_batches_per_scan,
        same_conversation_window_secs = config.same_conversation_window_secs,
        lease_secs = config.lease_secs,
        scan_interval_ms = config.scan_interval_ms,
        "确定性线程投影 Worker 已启动；不会逐消息调用 LLM"
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
        let mut projected = 0usize;
        let mut threads = 0usize;
        let mut relations = 0usize;
        let mut failed = false;
        for _ in 0..config.max_batches_per_scan {
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            let result = tokio::select! {
                result = runner.run_once() => Some(result),
                _ = shutdown_changed(&shutdown) => None,
            };
            let Some(result) = result else {
                break;
            };
            match result {
                Ok(Some(run)) => {
                    projected += run.events_projected;
                    threads += run.threads_created;
                    relations += run.relations_created;
                }
                Ok(None) => break,
                Err(error) => {
                    failed = true;
                    tracing::warn!(error = %error, "确定性线程投影批次失败，将退避重试");
                    break;
                }
            }
        }
        if failed {
            consecutive_errors = consecutive_errors.saturating_add(1);
        } else {
            consecutive_errors = 0;
        }
        if projected > 0 {
            tracing::debug!(
                events_projected = projected,
                threads_created = threads,
                relations_created = relations,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "确定性线程投影扫描完成"
            );
        } else {
            tracing::trace!("确定性线程投影暂无待处理事件");
        }
    }
    tracing::info!("确定性线程投影 Worker 已退出");
}

async fn shutdown_changed(shutdown: &Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn retry_delay_ms(config: &ThreadProjectionConfig, consecutive_errors: u32) -> u64 {
    let exponent = consecutive_errors.saturating_sub(1);
    let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
    config
        .retry_initial_ms
        .saturating_mul(multiplier)
        .min(config.retry_max_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StuckRunner;

    #[async_trait]
    impl ThreadProjectionRunner for StuckRunner {
        async fn run_once(&self) -> Result<Option<ThreadProjectionRun>, ThreadProjectionError> {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    #[test]
    fn retry_delay_is_exponential_and_capped() {
        let config = ThreadProjectionConfig {
            retry_initial_ms: 100,
            retry_max_ms: 500,
            ..ThreadProjectionConfig::default()
        };
        assert_eq!(retry_delay_ms(&config, 1), 100);
        assert_eq!(retry_delay_ms(&config, 2), 200);
        assert_eq!(retry_delay_ms(&config, 3), 400);
        assert_eq!(retry_delay_ms(&config, 4), 500);
        assert_eq!(retry_delay_ms(&config, 100), 500);
    }

    #[tokio::test]
    async fn shutdown_cancels_a_stuck_projection_call() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());
        let join = tokio::spawn(run_worker(
            Arc::new(StuckRunner),
            ThreadProjectionConfig {
                scan_interval_ms: 1,
                ..ThreadProjectionConfig::default()
            },
            Arc::clone(&shutdown),
            Arc::clone(&wake),
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.store(true, Ordering::Release);
        wake.notify_one();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("thread projection worker must stop while a database call is stuck")
            .expect("thread projection worker task must not panic");
    }
}
