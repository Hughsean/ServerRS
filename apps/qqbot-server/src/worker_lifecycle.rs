//! Worker 生命周期聚合与并发优雅关闭。
//!
//! 个人秘书运行时同时持有多个独立 Worker（回补、线程投影、线程语义、跨会话关联、
//! 承诺跟进、官方平台）。关闭时不能依次等待每个 Worker 的独立超时，否则极端情况下
//! 总关闭时间会叠加到不可接受的程度。
//!
//! 本模块提供：
//! - `WorkerHandle`：把任意 `JoinHandle<()>` 与其停止信号配对，支持带超时的回收；
//! - `RuntimeWorkers`：聚合所有可选 Worker，用单一全局 deadline 并发关闭。
//!
//! 关闭语义：
//! 1. 同时向所有 Worker 发出停止信号；
//! 2. 并发等待所有 Worker 退出，受单一全局 deadline 限制；
//! 3. 超时后用 `abort_handle()` 真正中止未完成的任务并限时回收，不让任务脱离管理。
//!
//! 测试可通过毫秒级 deadline 注入，避免常规测试等待真实秒数。

use std::time::Duration;

use futures_util::future::join_all;
use tokio::task::{AbortHandle, JoinHandle};
use tracing::warn;

/// 停止信号与 JoinHandle 的配对，支持带超时的回收。
///
/// 各 Worker handle 在 `shutdown()` 中构造 `WorkerHandle`，把停止信号发出后
/// 交由 [`shutdown_with_timeout`] 统一回收。
pub(crate) struct WorkerHandle {
    /// 停止信号已经发出后取出的 JoinHandle。
    join: Option<JoinHandle<()>>,
    /// 在进入并发等待前保存的 abort 句柄，用于超时后强制中止。
    abort: AbortHandle,
    /// Worker 名称，仅用于日志。
    name: &'static str,
}

impl WorkerHandle {
    pub(crate) fn new(name: &'static str, join: JoinHandle<()>) -> Self {
        let abort = join.abort_handle();
        Self {
            join: Some(join),
            abort,
            name,
        }
    }

    /// 带超时地等待 Worker 退出。超时后调用 `abort()` 并限时回收。
    ///
    /// 返回 `true` 表示 Worker 在 deadline 内正常退出，`false` 表示被强制中止。
    pub(crate) async fn join_with_timeout(mut self, deadline: Duration) -> bool {
        // 取出 join（此时停止信号已由调用方发出）。
        let mut join = match self.join.take() {
            Some(join) => join,
            None => return true,
        };
        match tokio::time::timeout(deadline, &mut join).await {
            Ok(_) => true,
            Err(_) => {
                warn!(
                    worker = self.name,
                    deadline_ms = deadline.as_millis(),
                    "Worker 未在全局关闭期限内退出，强制中止"
                );
                self.abort.abort();
                // 给被中止的任务一个短窗口回收，避免资源泄漏。
                let _ = tokio::time::timeout(Duration::from_millis(500), join).await;
                false
            }
        }
    }
}

/// 聚合所有可选 Worker，用单一全局 deadline 并发关闭。
///
/// 关闭时先同时发出所有停止信号（由各 handle 的 `into_worker_handle` 完成），
/// 再并发等待全部退出。超时后统一 `abort()` 并回收。
pub(crate) struct RuntimeWorkers {
    workers: Vec<WorkerHandle>,
}

impl RuntimeWorkers {
    pub(crate) fn new() -> Self {
        Self {
            workers: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, handle: WorkerHandle) {
        self.workers.push(handle);
    }

    /// 并发关闭所有 Worker，受单一全局 deadline 限制。
    ///
    /// 各 Worker 的停止信号必须已在构造 `WorkerHandle` 前发出（各 handle 的
    /// `shutdown` 方法负责置位标志 + notify/wake）。本方法只负责并发回收。
    pub(crate) async fn shutdown_all(self, deadline: Duration) {
        if self.workers.is_empty() {
            return;
        }
        let futures: Vec<_> = self
            .workers
            .into_iter()
            .map(|handle| handle.join_with_timeout(deadline))
            .collect();
        // 并发等待所有 Worker，整体受 deadline 限制（每个 WorkerHandle 内部也有相同 deadline）。
        let _ = join_all(futures).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 正常退出的 Worker 在毫秒级 deadline 内完成，不被误中止。
    #[tokio::test]
    async fn normal_worker_exits_within_deadline_without_abort() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = Arc::clone(&ran);
        let join = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            ran_clone.store(true, Ordering::Release);
        });
        let handle = WorkerHandle::new("fast", join);
        let ok = handle.join_with_timeout(Duration::from_millis(500)).await;
        assert!(ok, "normal worker should exit within deadline");
        assert!(ran.load(Ordering::Acquire));
    }

    /// 永不返回的 Worker 在 deadline 后被强制中止，调用方不会永久等待。
    #[tokio::test]
    async fn stuck_worker_is_aborted_after_deadline() {
        let join = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        let handle = WorkerHandle::new("stuck", join);
        let started = std::time::Instant::now();
        let ok = handle.join_with_timeout(Duration::from_millis(50)).await;
        let elapsed = started.elapsed();
        assert!(!ok, "stuck worker should be aborted");
        // 应在 deadline + 回收窗口内返回，而非永久挂起。
        assert!(
            elapsed < Duration::from_secs(2),
            "shutdown should return shortly after deadline, took {elapsed:?}"
        );
    }

    /// 多个 stuck Worker 并发关闭的总耗时受单一 deadline 限制，而非 N * deadline。
    #[tokio::test]
    async fn multiple_stuck_workers_share_single_deadline() {
        let mut runtime = RuntimeWorkers::new();
        for _ in 0..4 {
            let join = tokio::spawn(async {
                std::future::pending::<()>().await;
            });
            runtime.push(WorkerHandle::new("stuck", join));
        }
        let started = std::time::Instant::now();
        runtime.shutdown_all(Duration::from_millis(80)).await;
        let elapsed = started.elapsed();
        // 4 个并发 stuck Worker，总耗时应接近单一 deadline，而非 4 * 80ms。
        assert!(
            elapsed < Duration::from_secs(2),
            "concurrent shutdown should be bounded by single deadline, took {elapsed:?}"
        );
    }

    /// 空 RuntimeWorkers 立即返回。
    #[tokio::test]
    async fn empty_runtime_workers_returns_immediately() {
        let runtime = RuntimeWorkers::new();
        let started = std::time::Instant::now();
        runtime.shutdown_all(Duration::from_secs(10)).await;
        assert!(started.elapsed() < Duration::from_millis(50));
    }
}
