//! 结构化记忆候选提取 Worker。
//!
//! 独立可取消可退避的持久游标扫描：每次领取有界事件批次 -> 提取器生成
//! proposed 候选 -> 幂等提交 -> 推进游标。崩溃/重启从游标 + 租约恢复；
//! 连续错误按指数退避，shutdown 在提取卡住时也能取消（tokio::select）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use personal_secretary::{MemoryCandidateRun, MemoryCandidateUseCase, MemoryCandidateUseCaseError};
use tokio::task::JoinHandle;

use crate::config::MemoryCandidatesConfig;
use crate::worker_lifecycle::WorkerHandle;

#[async_trait]
trait MemoryCandidateRunner: Send + Sync {
    async fn run_once(&self) -> Result<Option<MemoryCandidateRun>, MemoryCandidateUseCaseError>;
}

#[async_trait]
impl MemoryCandidateRunner for MemoryCandidateUseCase {
    async fn run_once(&self) -> Result<Option<MemoryCandidateRun>, MemoryCandidateUseCaseError> {
        MemoryCandidateUseCase::run_once(self).await
    }
}

pub(crate) struct MemoryCandidatesHandle {
    shutdown: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    join: JoinHandle<()>,
}

impl MemoryCandidatesHandle {
    /// 发出停止信号并取出 JoinHandle，交由 [`WorkerHandle`] 统一带超时回收。
    pub(crate) fn signal_and_detach(self) -> WorkerHandle {
        self.shutdown.store(true, Ordering::Release);
        self.wake.notify_one();
        WorkerHandle::new("memory_candidates", self.join)
    }
}

pub(crate) fn spawn_memory_candidates_worker(
    use_case: Arc<MemoryCandidateUseCase>,
    config: MemoryCandidatesConfig,
) -> MemoryCandidatesHandle {
    let shutdown = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(tokio::sync::Notify::new());
    let join = tokio::spawn(run_worker(
        use_case,
        config,
        Arc::clone(&shutdown),
        Arc::clone(&wake),
    ));
    MemoryCandidatesHandle {
        shutdown,
        wake,
        join,
    }
}

async fn run_worker<R: MemoryCandidateRunner + 'static>(
    runner: Arc<R>,
    config: MemoryCandidatesConfig,
    shutdown: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
) {
    if !config.enabled {
        tracing::info!("记忆候选提取 Worker 已禁用（memory_candidates.enabled=false）");
        return;
    }
    tracing::info!(
        max_events_per_batch = config.max_events_per_batch,
        max_event_chars = config.max_event_chars,
        max_total_input_chars = config.max_total_input_chars,
        extractor_version = config.extractor_version,
        scan_interval_ms = config.scan_interval_ms,
        "记忆候选提取 Worker 已启动；仅生成可追溯 proposed 候选，Owner 批准后才落为记忆"
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
        let mut events_read = 0usize;
        let mut committed = 0u64;
        let mut skipped = 0u64;
        let mut invalidated = 0u64;
        let mut failed = false;
        for _ in 0..config.batch_size {
            let result = tokio::select! {
                result = runner.run_once() => Some(result),
                _ = shutdown_changed(&shutdown) => None,
            };
            let Some(result) = result else {
                break;
            };
            match result {
                Ok(Some(run)) => {
                    events_read += run.events_read;
                    committed += run.candidates_committed;
                    skipped += run.candidates_skipped;
                    invalidated += run.candidates_invalidated;
                }
                Ok(None) => break,
                Err(error) => {
                    failed = true;
                    tracing::warn!(error = %error, "记忆候选批次失败，将退避重试");
                    break;
                }
            }
        }
        consecutive_errors = if failed {
            consecutive_errors.saturating_add(1)
        } else {
            0
        };
        if events_read > 0 {
            tracing::debug!(
                events_read = events_read,
                candidates_committed = committed,
                candidates_skipped = skipped,
                candidates_invalidated = invalidated,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "记忆候选提取扫描完成"
            );
        } else {
            tracing::trace!("记忆候选暂无待处理事件");
        }
    }
    tracing::info!("记忆候选提取 Worker 已退出");
}

fn retry_delay_ms(config: &MemoryCandidatesConfig, consecutive_errors: u32) -> u64 {
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
    impl MemoryCandidateRunner for StuckRunner {
        async fn run_once(
            &self,
        ) -> Result<Option<MemoryCandidateRun>, MemoryCandidateUseCaseError> {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    #[test]
    fn retry_is_exponential_and_capped() {
        let config = MemoryCandidatesConfig {
            retry_initial_ms: 100,
            retry_max_ms: 500,
            ..MemoryCandidatesConfig::default()
        };
        assert_eq!(retry_delay_ms(&config, 1), 100);
        assert_eq!(retry_delay_ms(&config, 2), 200);
        assert_eq!(retry_delay_ms(&config, 4), 500);
    }

    #[tokio::test]
    async fn shutdown_cancels_stuck_candidate_extraction() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());
        let join = tokio::spawn(run_worker(
            Arc::new(StuckRunner),
            MemoryCandidatesConfig {
                scan_interval_ms: 1,
                ..MemoryCandidatesConfig::default()
            },
            Arc::clone(&shutdown),
            Arc::clone(&wake),
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.store(true, Ordering::Release);
        wake.notify_one();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("candidate worker must stop while extraction is stuck")
            .expect("candidate worker must not panic");
    }
}
