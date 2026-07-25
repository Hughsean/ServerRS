use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use personal_secretary::{ThreadLinkRun, ThreadLinkUseCase, ThreadLinkUseCaseError};
use tokio::task::JoinHandle;

use crate::config::ThreadLinksConfig;
use crate::worker_lifecycle::WorkerHandle;

#[async_trait]
trait ThreadLinkRunner: Send + Sync {
    async fn run_once(&self) -> Result<Option<ThreadLinkRun>, ThreadLinkUseCaseError>;
}

#[async_trait]
impl ThreadLinkRunner for ThreadLinkUseCase {
    async fn run_once(&self) -> Result<Option<ThreadLinkRun>, ThreadLinkUseCaseError> {
        ThreadLinkUseCase::run_once(self).await
    }
}

pub(crate) struct ThreadLinksHandle {
    shutdown: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    join: JoinHandle<()>,
}

impl ThreadLinksHandle {
    /// 发出停止信号并取出 JoinHandle，交由 [`WorkerHandle`] 统一带超时回收。
    pub(crate) fn signal_and_detach(self) -> WorkerHandle {
        self.shutdown.store(true, Ordering::Release);
        self.wake.notify_one();
        WorkerHandle::new("thread_links", self.join)
    }
}

pub(crate) fn spawn_thread_links_worker(
    use_case: Arc<ThreadLinkUseCase>,
    config: ThreadLinksConfig,
) -> ThreadLinksHandle {
    let shutdown = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(tokio::sync::Notify::new());
    let join = tokio::spawn(run_worker(
        use_case,
        config,
        Arc::clone(&shutdown),
        Arc::clone(&wake),
    ));
    ThreadLinksHandle {
        shutdown,
        wake,
        join,
    }
}

async fn run_worker<R: ThreadLinkRunner + 'static>(
    runner: Arc<R>,
    config: ThreadLinksConfig,
    shutdown: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
) {
    if !config.enabled {
        tracing::info!("跨会话线程关联候选 Worker 已禁用（thread_links.enabled=false）");
        return;
    }
    tracing::info!(
        max_events = config.max_events,
        max_total_chars = config.max_total_chars,
        max_batches_per_scan = config.max_batches_per_scan,
        "跨会话线程关联候选 Worker 已启动；仅保存强证据 proposed 候选，绝不自动合并"
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
        let mut hints = 0usize;
        let mut candidates = 0usize;
        let mut failed = false;
        for _ in 0..config.max_batches_per_scan {
            let result = tokio::select! {
                result = runner.run_once() => Some(result),
                _ = shutdown_changed(&shutdown) => None,
            };
            let Some(result) = result else { break };
            match result {
                Ok(Some(run)) => {
                    events += run.events_read;
                    hints += run.hints_created;
                    candidates += run.candidates_created;
                }
                Ok(None) => break,
                Err(error) => {
                    failed = true;
                    tracing::warn!(error = %error, "跨会话线程关联候选批次失败，将退避重试");
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
                hints_created = hints,
                candidates_created = candidates,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "跨会话线程关联候选扫描完成"
            );
        } else {
            tracing::trace!("跨会话线程关联候选暂无待处理事件");
        }
    }
    tracing::info!("跨会话线程关联候选 Worker 已退出");
}

fn retry_delay_ms(config: &ThreadLinksConfig, consecutive_errors: u32) -> u64 {
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
    impl ThreadLinkRunner for StuckRunner {
        async fn run_once(&self) -> Result<Option<ThreadLinkRun>, ThreadLinkUseCaseError> {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    #[tokio::test]
    async fn shutdown_cancels_stuck_link_scan() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());
        let join = tokio::spawn(run_worker(
            Arc::new(StuckRunner),
            ThreadLinksConfig {
                scan_interval_ms: 1,
                ..ThreadLinksConfig::default()
            },
            Arc::clone(&shutdown),
            Arc::clone(&wake),
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.store(true, Ordering::Release);
        wake.notify_one();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("thread links worker must stop")
            .expect("thread links worker must not panic");
    }
}
