//! 延迟 Reply 后台修复 Worker（EVT-007-MSG，Codex 复核 P1-1）。
//!
//! 实时路径（父事件重放/回补）解析 pending Reply 是最好情况；若父事件永不重放，
//! unresolved 子事件会永久滞留。本 Worker 周期调用
//! [`ReconcilePendingRepliesUseCase::run_one`]，有界领取 unresolved 候选（租约 +
//! SKIP LOCKED + 指数退避，跨重启安全），命中父事件时与主路径相同的回填与线程
//! 投影失效逻辑完成解析。
//!
//! 与回补 Worker 相同的生命周期约定：Notify 唤醒、AtomicBool 停止信号、连续错误
//! 指数退避、`WorkerHandle` 统一带超时回收。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use personal_secretary::{
    InboundEventStoreError, ReconcilePendingRepliesUseCase, ReconcileRunOutcome,
};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::config::ReplyReconcileConfig;
use crate::worker_lifecycle::WorkerHandle;

/// 修复用例抽象，便于 Worker 解耦与测试（用 FakeRunner 验证循环与关闭）。
#[async_trait]
pub(crate) trait ReplyReconcileRunner: Send + Sync {
    async fn run_one(&self) -> Result<ReconcileRunOutcome, InboundEventStoreError>;
}

#[async_trait]
impl ReplyReconcileRunner for ReconcilePendingRepliesUseCase {
    async fn run_one(&self) -> Result<ReconcileRunOutcome, InboundEventStoreError> {
        ReconcilePendingRepliesUseCase::run_one(self).await
    }
}

/// 对外句柄：唤醒和等待退出。
pub(crate) struct ReplyReconcileHandle {
    shutdown: Arc<AtomicBool>,
    wake: Arc<Notify>,
    join: JoinHandle<()>,
}

impl ReplyReconcileHandle {
    /// 发出停止信号并取出 JoinHandle，交由 [`WorkerHandle`] 统一带超时回收。
    /// 内部 `SHUTDOWN_GRACE`（10s）清理发生在 Worker 任务退出过程中；外层全局
    /// deadline 必须大于 10s，否则会抢先中止内部清理。
    pub(crate) fn signal_and_detach(self) -> WorkerHandle {
        self.shutdown.store(true, Ordering::Release);
        self.wake.notify_one();
        WorkerHandle::new("reply_reconcile", self.join)
    }
}

/// 启动独立延迟 Reply 修复 Worker。
pub(crate) fn spawn_reply_reconcile_worker<R: ReplyReconcileRunner + 'static>(
    use_case: Arc<R>,
    config: ReplyReconcileConfig,
) -> ReplyReconcileHandle {
    let wake = Arc::new(Notify::new());
    let shutdown = Arc::new(AtomicBool::new(false));
    let join = tokio::spawn(run_worker(
        use_case,
        config,
        Arc::clone(&wake),
        Arc::clone(&shutdown),
    ));
    ReplyReconcileHandle {
        shutdown,
        wake,
        join,
    }
}

async fn run_worker<R: ReplyReconcileRunner + 'static>(
    use_case: Arc<R>,
    config: ReplyReconcileConfig,
    wake: Arc<Notify>,
    shutdown: Arc<AtomicBool>,
) {
    if !config.enabled {
        tracing::info!("延迟 Reply 修复 Worker 已禁用（reply_reconcile.enabled=false）");
        return;
    }
    tracing::info!(
        batch_size = config.batch_size,
        lease_secs = config.lease_secs,
        retry_initial_ms = config.retry_initial_ms,
        retry_max_ms = config.retry_max_ms,
        "延迟 Reply 修复 Worker 已启动，与实时消息接收解耦"
    );

    let mut ticker = tokio::time::interval(Duration::from_millis(config.scan_interval_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    // 连续错误退避：数据库持续不可用时按指数退避推迟下一轮，避免热循环。
    let mut consecutive_errors: u32 = 0;

    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        // 错误退避是下一次扫描前不可绕过的最短等待时间；周期 tick 与 wake 不能
        // 提前结束退避，否则 retry_max_ms 大于扫描间隔时仍会被固定 ticker 绕过。
        // 退避只推迟本轮执行，不跳过本轮：错误计数由 run 结果归零/递增。
        let backoff = backoff_duration(&config, consecutive_errors);
        if backoff > Duration::ZERO {
            tracing::warn!(
                consecutive_errors = consecutive_errors,
                backoff_ms = backoff.as_millis() as u64,
                "延迟 Reply 修复 Worker 连续失败，按退避推迟本轮"
            );
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = shutdown_changed(&shutdown) => break,
            }
        }
        if shutdown.load(Ordering::Acquire) {
            break;
        }

        match use_case.run_one().await {
            Ok(outcome) => {
                consecutive_errors = 0;
                tracing::debug!(
                    claimed = outcome.claimed,
                    resolved = outcome.resolved,
                    still_pending = outcome.still_pending,
                    "延迟 Reply 修复轮次完成"
                );
            }
            Err(error) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                tracing::error!(
                    error_code = "reply_reconcile_run_failed",
                    consecutive_errors = consecutive_errors,
                    "延迟 Reply 修复轮次失败"
                );
                let _ = &error;
            }
        }

        tokio::select! {
            _ = ticker.tick() => {}
            _ = wake.notified() => {}
            _ = shutdown_changed(&shutdown) => break,
        }
    }
    tracing::info!("延迟 Reply 修复 Worker 已退出");
}

async fn shutdown_changed(shutdown: &Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 连续错误指数退避：`retry_initial_ms * 2^errors`，封顶 `retry_max_ms`。
fn backoff_duration(config: &ReplyReconcileConfig, consecutive_errors: u32) -> Duration {
    if consecutive_errors == 0 {
        return Duration::ZERO;
    }
    let exponent = consecutive_errors.saturating_sub(1).min(20);
    let millis = config
        .retry_initial_ms
        .saturating_mul(1u64 << exponent)
        .min(config.retry_max_ms);
    Duration::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_secretary::ReconcileRunOutcome;
    use std::sync::atomic::AtomicU32;

    fn test_config() -> ReplyReconcileConfig {
        ReplyReconcileConfig {
            enabled: true,
            scan_interval_ms: 10_000,
            batch_size: 10,
            lease_secs: 60,
            retry_initial_ms: 1,
            retry_max_ms: 2,
        }
    }

    /// 可观测 FakeRunner：记录调用次数，可注入错误验证退避路径。
    struct FakeRunner {
        calls: Arc<AtomicU32>,
        fail_then_recover: Arc<AtomicU32>,
    }

    #[async_trait]
    impl ReplyReconcileRunner for FakeRunner {
        async fn run_one(&self) -> Result<ReconcileRunOutcome, InboundEventStoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let remaining = self.fail_then_recover.load(Ordering::SeqCst);
            if remaining > 0 {
                self.fail_then_recover.fetch_sub(1, Ordering::SeqCst);
                return Err(InboundEventStoreError::Database("injected failure".into()));
            }
            Ok(ReconcileRunOutcome {
                claimed: 2,
                resolved: 1,
                still_pending: 1,
            })
        }
    }

    #[tokio::test]
    async fn worker_runs_until_shutdown() {
        let calls = Arc::new(AtomicU32::new(0));
        let runner = Arc::new(FakeRunner {
            calls: Arc::clone(&calls),
            fail_then_recover: Arc::new(AtomicU32::new(0)),
        });
        let handle = spawn_reply_reconcile_worker(runner, test_config());
        // 等第一轮完成（scan_interval 10s > 测试时长，只有第一轮会被执行）。
        tokio::time::timeout(Duration::from_secs(5), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("worker must run at least one round");
        let detach = handle.signal_and_detach();
        assert!(
            detach.join_with_timeout(Duration::from_secs(5)).await,
            "worker must exit promptly on shutdown signal"
        );
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "worker must have executed at least one reconciliation round"
        );
    }

    #[tokio::test]
    async fn worker_recovers_from_errors_with_backoff() {
        let calls = Arc::new(AtomicU32::new(0));
        let runner = Arc::new(FakeRunner {
            calls: Arc::clone(&calls),
            fail_then_recover: Arc::new(AtomicU32::new(1)),
        });
        let handle = spawn_reply_reconcile_worker(runner, test_config());
        tokio::time::timeout(Duration::from_secs(5), async {
            while calls.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("worker must retry after failure");
        let detach = handle.signal_and_detach();
        assert!(
            detach.join_with_timeout(Duration::from_secs(5)).await,
            "worker must exit promptly on shutdown signal"
        );
        assert!(
            calls.load(Ordering::SeqCst) >= 2,
            "worker must retry after a failed round"
        );
    }

    #[tokio::test]
    async fn disabled_worker_exits_immediately() {
        let calls = Arc::new(AtomicU32::new(0));
        let runner = Arc::new(FakeRunner {
            calls: Arc::clone(&calls),
            fail_then_recover: Arc::new(AtomicU32::new(0)),
        });
        let mut config = test_config();
        config.enabled = false;
        let handle = spawn_reply_reconcile_worker(runner, config);
        let detach = handle.signal_and_detach();
        assert!(
            detach.join_with_timeout(Duration::from_secs(5)).await,
            "disabled worker must exit promptly"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "disabled worker must not run"
        );
    }

    #[test]
    fn backoff_grows_exponentially_and_caps_at_max() {
        let config = ReplyReconcileConfig {
            enabled: true,
            scan_interval_ms: 1000,
            batch_size: 1,
            lease_secs: 60,
            retry_initial_ms: 100,
            retry_max_ms: 500,
        };
        assert_eq!(backoff_duration(&config, 0), Duration::ZERO);
        assert_eq!(backoff_duration(&config, 1).as_millis(), 100);
        assert_eq!(backoff_duration(&config, 2).as_millis(), 200);
        assert_eq!(backoff_duration(&config, 3).as_millis(), 400);
        // 封顶 retry_max_ms，防止溢出与无限增长。
        assert_eq!(backoff_duration(&config, 4).as_millis(), 500);
        assert_eq!(backoff_duration(&config, 100).as_millis(), 500);
    }
}
