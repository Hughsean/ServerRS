//! 历史回补独立 Worker。
//!
//! 与实时 WebSocket 接收解耦：不在 NapCat 事件回调中等待 HTTP 历史分页，不阻塞实时
//! `mpsc` 入站队列。Worker 周期性领取 `uncertain` Gap 并驱动回补用例，NapCat 重新连接
//! 成功后通过唤醒通道立即触发一次扫描，服务关闭时通过取消标志优雅回收。
//!
//! 行为约束：
//! - 多个回补任务通过 `JoinSet` 真正并发执行，受 `max_concurrency` 限制；
//! - 关闭信号通过 `AtomicBool` 取消标志传入循环，`shutdown()` 不会永久挂起——即使
//!   在飞任务或过期运行领取卡住，也会取消等待并中止在飞任务；
//! - 数据库或 NapCat 持续报错时遵守配置退避（`retry_initial_ms`/`retry_max_ms`），
//!   不会形成无退避热循环；
//! - MySQL 或 NapCat 暂时不可用时保持可恢复状态（Gap 仍为 uncertain/backfilling）；
//! - 结构化日志包含 run_id/gap_id/completeness/pages/accepted/duplicates/budget_exhausted，
//!   但绝不记录消息正文、Token 或完整历史响应。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use personal_secretary::{BackfillError, BackfillGapUseCase, BackfillOutcome, ClaimedGap};
use tokio::sync::Notify;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

use crate::config::BackfillConfig;

/// 周期扫描间隔：没有重连唤醒时，定期检查是否有可领取的 uncertain Gap。
const IDLE_SCAN_INTERVAL: Duration = Duration::from_secs(30);

/// 关闭时等待在飞任务自行退出的最长时间。超过后中止所有在飞任务强制退出，其 Gap
/// 因租约过期可由 `reclaim_expired` 在下次启动恢复。该上限保证 `shutdown()` 不会
/// 因单个卡住的 HTTP 请求而永久挂起。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// 回补用例抽象，便于 Worker 解耦与测试（用 FakeRunner 验证并发与关闭）。
#[async_trait]
pub(crate) trait BackfillRunner: Send + Sync {
    async fn run_one(&self) -> Result<Option<BackfillOutcome>, BackfillError>;
    async fn reclaim_expired(&self, limit: u32) -> Result<Vec<ClaimedGap>, BackfillError>;
    async fn resume_claimed(&self, claimed: ClaimedGap) -> Result<BackfillOutcome, BackfillError>;
}

#[async_trait]
impl BackfillRunner for BackfillGapUseCase {
    async fn run_one(&self) -> Result<Option<BackfillOutcome>, BackfillError> {
        BackfillGapUseCase::run_one(self).await
    }
    async fn reclaim_expired(&self, limit: u32) -> Result<Vec<ClaimedGap>, BackfillError> {
        BackfillGapUseCase::reclaim_expired(self, limit).await
    }
    async fn resume_claimed(&self, claimed: ClaimedGap) -> Result<BackfillOutcome, BackfillError> {
        BackfillGapUseCase::resume_claimed(self, claimed).await
    }
}

/// 对外句柄：用于唤醒（重连成功后）和等待退出。
pub(crate) struct BackfillHandle {
    wake: Arc<Notify>,
    shutdown: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

impl BackfillHandle {
    /// NapCat 重新连接成功后唤醒 Worker 立即扫描一次回补。
    #[allow(dead_code)]
    pub(crate) fn wake(&self) {
        self.wake.notify_one();
    }

    /// 暴露共享唤醒通知，供 ConnectionObserver 在重连成功时唤醒 Worker。
    pub(crate) fn wake_notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.wake)
    }

    /// 服务关闭时优雅等待 Worker 退出。置位取消标志并唤醒，确保循环不会永久挂起。
    pub(crate) async fn shutdown(self) {
        self.shutdown.store(true, Ordering::Release);
        self.wake.notify_one();
        let _ = self.join.await;
    }
}

/// 启动独立回补 Worker。返回句柄供运行时唤醒和回收。
pub(crate) fn spawn_backfill_worker<R: BackfillRunner + 'static>(
    use_case: Arc<R>,
    config: BackfillConfig,
) -> BackfillHandle {
    let wake = Arc::new(Notify::new());
    let shutdown = Arc::new(AtomicBool::new(false));
    let join = tokio::spawn(run_worker(
        use_case,
        config,
        Arc::clone(&wake),
        Arc::clone(&shutdown),
    ));
    BackfillHandle {
        wake,
        shutdown,
        join,
    }
}

async fn run_worker<R: BackfillRunner + 'static>(
    use_case: Arc<R>,
    config: BackfillConfig,
    wake: Arc<Notify>,
    shutdown: Arc<AtomicBool>,
) {
    if !config.enabled {
        tracing::info!("历史回补 Worker 已禁用（backfill.enabled=false）");
        return;
    }

    tracing::info!(
        max_concurrency = config.max_concurrency,
        page_size = config.page_size,
        max_pages_per_scope = config.max_pages_per_scope,
        max_events_per_run = config.max_events_per_run,
        lease_secs = config.lease_secs,
        retry_initial_ms = config.retry_initial_ms,
        retry_max_ms = config.retry_max_ms,
        idle_scan_interval_secs = IDLE_SCAN_INTERVAL.as_secs(),
        "历史回补 Worker 已启动，与实时 WebSocket 接收解耦"
    );

    let max_concurrency = config.max_concurrency.max(1) as usize;
    let mut ticker = tokio::time::interval(IDLE_SCAN_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    // 连续错误退避：数据库或 NapCat 持续不可用时，按指数退避推迟下一次扫描，
    // 避免无退避热循环。一旦某轮扫描成功处理或恢复，退避计数归零。
    let mut consecutive_errors: u32 = 0;

    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }

        // 错误退避是下一次扫描前不可绕过的最短等待时间。周期 tick 和重连 wake 不能
        // 提前结束退避，否则 retry_max_ms 大于 30s 时仍会被固定 ticker 绕过。
        let backoff = backoff_duration(&config, consecutive_errors);

        if backoff > Duration::ZERO {
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = shutdown_changed(&shutdown) => break,
            }
        } else {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = wake.notified() => {
                    tracing::debug!("历史回补 Worker 收到重连唤醒，立即扫描");
                }
                _ = shutdown_changed(&shutdown) => break,
            }
        }
        if shutdown.load(Ordering::Acquire) {
            break;
        }

        let scan_result = scan_once(&use_case, max_concurrency, &shutdown).await;
        for outcome in &scan_result.outcomes {
            tracing::info!(
                backfill_run_id = %outcome.run_id.as_str(),
                gap_id = %outcome.gap_id.as_str(),
                completeness = outcome.completeness.as_str(),
                gap_target_status = outcome.gap_target_status.as_str(),
                pages_read = outcome.evidence.scopes.iter().map(|s| s.pages_read).sum::<u32>(),
                accepted = outcome.evidence.scopes.iter().map(|s| s.accepted).sum::<u32>(),
                duplicates = outcome.evidence.scopes.iter().map(|s| s.duplicates).sum::<u32>(),
                budget_exhausted = outcome.evidence.budget_exhausted,
                "历史回补运行已结束"
            );
        }

        // 根据本轮结果调整退避计数：有错误递增，无错误归零。
        if scan_result.had_error {
            consecutive_errors = consecutive_errors.saturating_add(1);
            let next_backoff = backoff_duration(&config, consecutive_errors);
            tracing::warn!(
                consecutive_errors,
                next_backoff_ms = next_backoff.as_millis() as u64,
                "历史回补本轮遇到错误，将按退避推迟下一次扫描"
            );
        } else {
            consecutive_errors = 0;
        }
    }

    tracing::info!("历史回补 Worker 已退出");
}

/// 根据配置和连续错误次数计算指数退避时长。
///
/// 公式：`min(retry_initial_ms * 2^(n-1), retry_max_ms)`，其中 `n` 为连续错误次数。
/// 第一次错误使用 `retry_initial_ms`，`n=0` 时返回零。
fn backoff_duration(config: &BackfillConfig, consecutive_errors: u32) -> Duration {
    if consecutive_errors == 0 {
        return Duration::ZERO;
    }
    let initial = config.retry_initial_ms;
    let max = config.retry_max_ms;
    // 2^(n-1) * initial，用 saturating_mul 防止溢出。
    let exponent = consecutive_errors.saturating_sub(1);
    let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
    let delay = initial.saturating_mul(multiplier).min(max);
    Duration::from_millis(delay)
}

/// 一次扫描的结果：完成的运行结果 + 是否遇到需要退避的错误。
struct ScanResult {
    outcomes: Vec<BackfillOutcome>,
    had_error: bool,
}

/// 执行一次扫描：领取过期运行并与新 Gap 一起并发处理，受 `max_concurrency` 限制。
///
/// 通过 `JoinSet` 真正并发 spawn 多个 `run_one` 任务（而非串行 await），使 `max_concurrency`
/// 产生实际并发吞吐。`run_one` 内部原子领取保证不会重复处理同一 Gap。
///
/// 过期运行的领取与历史处理拆开：领取只返回有界 Claim，实际恢复进入同一个 `JoinSet`。
/// 这样合法的长回补不会被固定 5 秒超时误杀，同时关闭仍可中止所有在飞任务。
async fn scan_once<R: BackfillRunner + 'static>(
    use_case: &Arc<R>,
    max_concurrency: usize,
    shutdown: &Arc<AtomicBool>,
) -> ScanResult {
    let mut outcomes = Vec::new();
    let mut had_error = false;

    // 仅领取最多 max_concurrency 个过期运行。数据库调用与关闭竞速，但不对完整恢复过程
    // 施加固定短超时；领取成功后的历史处理统一进入下方 JoinSet。
    let reclaimed_result = if shutdown.load(Ordering::Acquire) {
        Ok(Vec::new())
    } else {
        tokio::select! {
            r = use_case.reclaim_expired(max_concurrency as u32) => r,
            _ = shutdown_changed(shutdown) => {
                Ok(Vec::new())
            }
        }
    };
    let reclaimed = match reclaimed_result {
        Ok(claimed) => claimed,
        Err(error) => {
            had_error = true;
            tracing::warn!(
                error = %error,
                "历史回补过期运行领取失败，将在退避后重试"
            );
            Vec::new()
        }
    };

    let mut tasks: JoinSet<Result<(Option<BackfillOutcome>, Instant), BackfillError>> =
        JoinSet::new();
    for claimed in reclaimed {
        let runner = Arc::clone(use_case);
        let started = Instant::now();
        tasks.spawn(async move {
            let outcome = runner.resume_claimed(claimed).await?;
            Ok((Some(outcome), started))
        });
    }
    // 过期运行领取失败时不继续向同一不可用存储发起新领取，交给外层统一退避。
    let mut drained = had_error;

    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        // 填充到并发上限：每个任务内部调用 run_one 领取一个 Gap。
        while !drained && tasks.len() < max_concurrency {
            let runner = Arc::clone(use_case);
            let started = Instant::now();
            tasks.spawn(async move {
                let outcome = runner.run_one().await?;
                Ok((outcome, started))
            });
        }
        if tasks.is_empty() {
            break;
        }

        // 关闭时不再等待新任务完成，直接进入回收阶段。
        if shutdown.load(Ordering::Acquire) {
            break;
        }

        // 等待下一个任务完成，同时监听关闭信号。
        // 关闭信号优先：一旦置位立即跳出循环进入回收，不等待卡住的任务。
        tokio::select! {
            joined = tasks.join_next() => {
                match joined {
                    Some(Ok(Ok((Some(outcome), started)))) => {
                        let elapsed_ms = started.elapsed().as_millis() as u64;
                        tracing::debug!(
                            backfill_run_id = %outcome.run_id.as_str(),
                            gap_id = %outcome.gap_id.as_str(),
                            elapsed_ms,
                            completeness = outcome.completeness.as_str(),
                            "历史回补并发任务完成"
                        );
                        outcomes.push(outcome);
                    }
                    Some(Ok(Ok((None, _started)))) => {
                        // 某任务未领到 Gap：停止 spawn 新任务，但让在飞任务完成。
                        drained = true;
                    }
                    Some(Ok(Err(error))) => {
                        had_error = true;
                        // 停止 spawn 新任务：错误退避在 run_worker 的扫描间隔之间进行，
                        // 而非在单次 scan_once 内部热循环。让在飞任务继续完成。
                        drained = true;
                        tracing::warn!(
                            error = %error,
                            "历史回补运行失败，Gap 保持 uncertain 或 backfilling，将在下次扫描重试"
                        );
                    }
                    Some(Err(error)) => {
                        had_error = true;
                        drained = true;
                        tracing::warn!(error = %error, "历史回补任务 panic");
                    }
                    None => break,
                }
            }
            _ = shutdown_changed(shutdown) => {
                // 关闭信号置位：跳出循环进入回收阶段。
                break;
            }
        }
    }

    // 关闭时中止在飞任务（其 Gap 因租约过期可由 reclaim_expired 恢复）；
    // 否则正常排空在飞任务。无论哪种情况，都在 SHUTDOWN_GRACE 内完成，避免永久挂起。
    if shutdown.load(Ordering::Acquire) {
        tracing::info!(
            in_flight = tasks.len(),
            "历史回补 Worker 关闭中，中止在飞任务（其 Gap 将由租约过期恢复）"
        );
        tasks.abort_all();
        // 等待被中止的任务清理句柄，限时 SHUTDOWN_GRACE。
        let _ = tokio::time::timeout(SHUTDOWN_GRACE, drain_joinset(&mut tasks)).await;
    } else {
        // 正常退出（drained 且无在飞）时 drain_joinset 立即返回。
        drain_joinset(&mut tasks).await;
    }
    ScanResult {
        outcomes,
        had_error,
    }
}

/// 排空 JoinSet 中剩余的句柄，防止任务资源泄漏。
async fn drain_joinset<T: 'static>(tasks: &mut JoinSet<T>) {
    while tasks.join_next().await.is_some() {}
}

/// 异步等待关闭标志置位。使用短轮询（100ms）检测 `AtomicBool` 变化，
/// 使 `scan_once` 内部的 `join_next` 等待能及时响应关闭信号。
async fn shutdown_changed(shutdown: &Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_secretary::InboundEventStoreError;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn config() -> BackfillConfig {
        BackfillConfig {
            enabled: true,
            page_size: 10,
            max_pages_per_scope: 5,
            max_events_per_run: 100,
            max_concurrency: 1,
            lease_secs: 60,
            retry_initial_ms: 1,
            retry_max_ms: 2,
        }
    }

    /// 可观测的 FakeRunner：记录并发在飞数，证明 max_concurrency 产生真实并发。
    struct FakeRunner {
        in_flight: Arc<AtomicU32>,
        max_observed: Arc<AtomicU32>,
        remaining: Arc<AtomicU32>,
        block_ms: u64,
    }

    impl FakeRunner {
        fn new(total: u32, block_ms: u64) -> (Self, Arc<AtomicU32>) {
            let in_flight = Arc::new(AtomicU32::new(0));
            let max_observed = Arc::new(AtomicU32::new(0));
            let runner = Self {
                in_flight: Arc::clone(&in_flight),
                max_observed: Arc::clone(&max_observed),
                remaining: Arc::new(AtomicU32::new(total)),
                block_ms,
            };
            (runner, max_observed)
        }

        fn record_max(&self, cur: u32) {
            let mut current = self.max_observed.load(Ordering::Acquire);
            while cur > current {
                match self.max_observed.compare_exchange(
                    current,
                    cur,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(actual) => current = actual,
                }
            }
        }
    }

    #[async_trait]
    impl BackfillRunner for FakeRunner {
        async fn run_one(&self) -> Result<Option<BackfillOutcome>, BackfillError> {
            // 原子递减剩余配额；为 0 时返回 None。
            let prev = self.remaining.fetch_sub(1, Ordering::AcqRel);
            if prev == 0 {
                self.remaining.fetch_add(1, Ordering::AcqRel);
                return Ok(None);
            }
            let cur = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
            self.record_max(cur);
            if self.block_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.block_ms)).await;
            }
            self.in_flight.fetch_sub(1, Ordering::AcqRel);
            Ok(Some(fake_outcome()))
        }

        async fn reclaim_expired(&self, _limit: u32) -> Result<Vec<ClaimedGap>, BackfillError> {
            Ok(Vec::new())
        }

        async fn resume_claimed(
            &self,
            _claimed: ClaimedGap,
        ) -> Result<BackfillOutcome, BackfillError> {
            Ok(fake_outcome())
        }
    }

    /// 始终返回错误的 FakeRunner，用于验证退避和错误处理。可选择阻塞模拟卡住的请求。
    struct ErrorRunner {
        block_ms: u64,
        calls: Arc<AtomicU32>,
    }

    #[async_trait]
    impl BackfillRunner for ErrorRunner {
        async fn run_one(&self) -> Result<Option<BackfillOutcome>, BackfillError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            if self.block_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.block_ms)).await;
            }
            Err(BackfillError::State(InboundEventStoreError::Unavailable))
        }

        async fn reclaim_expired(&self, _limit: u32) -> Result<Vec<ClaimedGap>, BackfillError> {
            Ok(Vec::new())
        }

        async fn resume_claimed(
            &self,
            _claimed: ClaimedGap,
        ) -> Result<BackfillOutcome, BackfillError> {
            Err(BackfillError::State(InboundEventStoreError::Unavailable))
        }
    }

    /// 永不返回的 FakeRunner，模拟卡住的 NapCat HTTP 请求。用于验证关闭时在飞任务
    /// 不会阻止退出。
    struct StuckRunner;

    #[async_trait]
    impl BackfillRunner for StuckRunner {
        async fn run_one(&self) -> Result<Option<BackfillOutcome>, BackfillError> {
            // 永远挂起，模拟卡住的 HTTP 请求。关闭必须通过 abort 退出，而非等待此返回。
            std::future::pending::<()>().await;
            unreachable!()
        }

        async fn reclaim_expired(&self, _limit: u32) -> Result<Vec<ClaimedGap>, BackfillError> {
            Ok(Vec::new())
        }

        async fn resume_claimed(
            &self,
            _claimed: ClaimedGap,
        ) -> Result<BackfillOutcome, BackfillError> {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    /// 恢复处理耗时超过旧 5 秒上限的 Runner，验证合法长任务不会被固定超时取消。
    struct SlowResumeRunner {
        claimed: AtomicBool,
        delay: Duration,
    }

    #[async_trait]
    impl BackfillRunner for SlowResumeRunner {
        async fn run_one(&self) -> Result<Option<BackfillOutcome>, BackfillError> {
            Ok(None)
        }

        async fn reclaim_expired(&self, _limit: u32) -> Result<Vec<ClaimedGap>, BackfillError> {
            if self.claimed.swap(true, Ordering::AcqRel) {
                Ok(Vec::new())
            } else {
                Ok(vec![fake_claimed_gap()])
            }
        }

        async fn resume_claimed(
            &self,
            _claimed: ClaimedGap,
        ) -> Result<BackfillOutcome, BackfillError> {
            tokio::time::sleep(self.delay).await;
            Ok(fake_outcome())
        }
    }

    /// 领取过期运行的数据库调用永不返回，验证关闭可以取消领取等待。
    struct StuckReclaimRunner;

    #[async_trait]
    impl BackfillRunner for StuckReclaimRunner {
        async fn run_one(&self) -> Result<Option<BackfillOutcome>, BackfillError> {
            Ok(None)
        }

        async fn reclaim_expired(&self, _limit: u32) -> Result<Vec<ClaimedGap>, BackfillError> {
            std::future::pending::<()>().await;
            unreachable!()
        }

        async fn resume_claimed(
            &self,
            _claimed: ClaimedGap,
        ) -> Result<BackfillOutcome, BackfillError> {
            unreachable!()
        }
    }

    fn fake_outcome() -> BackfillOutcome {
        use personal_secretary::{
            BackfillEvidence, BackfillRunId, HistoryCompleteness, IngestionGapId,
            IngestionGapStatus,
        };
        BackfillOutcome {
            run_id: BackfillRunId::new("run").unwrap(),
            gap_id: IngestionGapId::new("gap").unwrap(),
            completeness: HistoryCompleteness::ProvenComplete,
            evidence: BackfillEvidence::default(),
            gap_target_status: IngestionGapStatus::VerifiedComplete,
            gap_reason: None,
            failure_class: None,
        }
    }

    fn fake_claimed_gap() -> ClaimedGap {
        use personal_secretary::{
            BackfillLeaseToken, ConnectionEpochId, MessageSource, SourceAccountRef,
        };
        ClaimedGap {
            run_id: personal_secretary::BackfillRunId::new("run").unwrap(),
            lease_token: BackfillLeaseToken::new("lease").unwrap(),
            gap_id: personal_secretary::IngestionGapId::new("gap").unwrap(),
            account: SourceAccountRef::new(MessageSource::NapCat, "account").unwrap(),
            connection_epoch_id: ConnectionEpochId::new("epoch").unwrap(),
            is_resume: true,
        }
    }

    #[tokio::test]
    async fn max_concurrency_produces_real_concurrency() {
        // 8 个 Gap、并发上限 4、每个任务阻塞 40ms：若真并发，max_observed 应 >= 2
        //（串行实现只会到 1）。这是验证修复"假并发"的失败测试。
        let (runner, max_observed) = FakeRunner::new(8, 40);
        let use_case = Arc::new(runner);
        let shutdown = Arc::new(AtomicBool::new(false));
        let result = scan_once(&use_case, 4, &shutdown).await;

        let observed = max_observed.load(Ordering::Acquire);
        assert!(
            observed >= 2,
            "max_concurrency must produce real concurrency, observed max in-flight = {observed}"
        );
        assert!(!result.had_error);
        assert_eq!(result.outcomes.len(), 8);
    }

    #[tokio::test]
    async fn shutdown_does_not_hang() {
        // FakeRunner 立即返回 None（无 Gap）。spawn Worker 后立即 shutdown 必须在超时内退出，
        // 证明取消标志让循环可退出（修复"关闭永久等待"）。
        let (runner, _max) = FakeRunner::new(0, 0);
        let use_case = Arc::new(runner);
        let mut cfg = config();
        cfg.max_concurrency = 2;
        let handle = spawn_backfill_worker(use_case, cfg);

        tokio::time::timeout(Duration::from_secs(2), handle.shutdown())
            .await
            .expect("shutdown must not hang");
    }

    #[tokio::test]
    async fn shutdown_exits_with_in_flight_stuck_task() {
        // StuckRunner 的 run_one 永远挂起（模拟卡住的 NapCat HTTP 请求）。
        // spawn Worker 后立即 shutdown 必须在 SHUTDOWN_GRACE + 余量内退出，
        // 证明在飞任务永不返回时关闭不会永久等待。这是 P1a 的核心修复验证。
        let use_case = Arc::new(StuckRunner);
        let cfg = config();
        let handle = spawn_backfill_worker(use_case, cfg);

        // 给 Worker 一点时间进入 scan_once 并 spawn 卡住的任务。
        tokio::time::sleep(Duration::from_millis(100)).await;

        // SHUTDOWN_GRACE 是 10s；关闭应在远低于此的时间完成（abort 后清理句柄）。
        // 但保留足够余量应对调度延迟。
        tokio::time::timeout(Duration::from_secs(15), handle.shutdown())
            .await
            .expect("shutdown must not hang even when an in-flight task never returns");
    }

    #[tokio::test]
    async fn legitimate_resume_longer_than_five_seconds_completes() {
        let use_case = Arc::new(SlowResumeRunner {
            claimed: AtomicBool::new(false),
            delay: Duration::from_millis(5_100),
        });
        let shutdown = Arc::new(AtomicBool::new(false));

        let result =
            tokio::time::timeout(Duration::from_secs(7), scan_once(&use_case, 1, &shutdown))
                .await
                .expect("a legitimate long resume must finish without a fixed five-second cutoff");

        assert!(!result.had_error);
        assert_eq!(result.outcomes.len(), 1);
    }

    #[tokio::test]
    async fn shutdown_cancels_stuck_expired_claim() {
        let handle = spawn_backfill_worker(Arc::new(StuckReclaimRunner), config());
        tokio::time::sleep(Duration::from_millis(100)).await;

        tokio::time::timeout(Duration::from_secs(2), handle.shutdown())
            .await
            .expect("shutdown must cancel a stuck expired-run claim");
    }

    #[tokio::test]
    async fn scan_once_reports_error_and_does_not_hot_loop() {
        // ErrorRunner 始终返回错误。单次 scan_once 应报告 had_error=true 且不产生结果。
        // run_worker 层面会根据 had_error 递增退避计数，避免热循环。
        let use_case = Arc::new(ErrorRunner {
            block_ms: 0,
            calls: Arc::new(AtomicU32::new(0)),
        });
        let shutdown = Arc::new(AtomicBool::new(false));
        let result = scan_once(&use_case, 2, &shutdown).await;

        assert!(result.had_error, "scan must report error from ErrorRunner");
        assert!(
            result.outcomes.is_empty(),
            "no outcomes from an always-erroring runner"
        );
    }

    #[tokio::test]
    async fn backoff_duration_grows_exponentially_and_caps_at_max() {
        // 验证退避公式：min(initial * 2^(n-1), max)。
        let cfg = BackfillConfig {
            retry_initial_ms: 100,
            retry_max_ms: 1_000,
            ..config()
        };
        assert_eq!(backoff_duration(&cfg, 0), Duration::ZERO);
        assert_eq!(backoff_duration(&cfg, 1), Duration::from_millis(100));
        assert_eq!(backoff_duration(&cfg, 2), Duration::from_millis(200));
        assert_eq!(backoff_duration(&cfg, 3), Duration::from_millis(400));
        assert_eq!(backoff_duration(&cfg, 4), Duration::from_millis(800));
        // 100 * 2^4 = 1600 > 1000 => 封顶。
        assert_eq!(backoff_duration(&cfg, 5), Duration::from_millis(1_000));
        assert_eq!(backoff_duration(&cfg, 100), Duration::from_millis(1_000));
    }

    #[tokio::test]
    async fn worker_backs_off_on_consecutive_errors() {
        // ErrorRunner 始终报错。spawn Worker 后，应在退避间隔之间暂停而非热循环。
        // 使用较大的退避值确保 Worker 在 300ms 内只运行少量轮次。
        let calls = Arc::new(AtomicU32::new(0));
        let use_case = Arc::new(ErrorRunner {
            block_ms: 0,
            calls: Arc::clone(&calls),
        });
        let cfg = BackfillConfig {
            retry_initial_ms: 100,
            retry_max_ms: 500,
            ..config()
        };
        let handle = spawn_backfill_worker(use_case, cfg);

        // 等待 300ms，让 Worker 运行几轮。
        // 第 1 轮：backoff=0（consecutive_errors=0），立即扫描。
        // 第 2 轮：首次错误后等待 100ms；第 3 轮再等待 200ms。
        // 250ms 内应恰好执行两轮，不能被 30s ticker 或 wake 绕过退避。
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            calls.load(Ordering::Acquire),
            2,
            "consecutive errors must wait for the configured exponential backoff"
        );

        // 关闭 Worker。能在期限内退出即证明未卡死。
        tokio::time::timeout(Duration::from_secs(15), handle.shutdown())
            .await
            .expect("worker with backoff must shut down cleanly");
    }

    #[tokio::test]
    async fn reconnect_wake_does_not_bypass_error_backoff() {
        let calls = Arc::new(AtomicU32::new(0));
        let use_case = Arc::new(ErrorRunner {
            block_ms: 0,
            calls: Arc::clone(&calls),
        });
        let cfg = BackfillConfig {
            retry_initial_ms: 500,
            retry_max_ms: 500,
            ..config()
        };
        let handle = spawn_backfill_worker(use_case, cfg);

        tokio::time::timeout(Duration::from_secs(1), async {
            while calls.load(Ordering::Acquire) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the first scan must start");
        handle.wake();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            calls.load(Ordering::Acquire),
            1,
            "a reconnect wake must not bypass the mandatory error backoff"
        );

        tokio::time::timeout(Duration::from_secs(2), handle.shutdown())
            .await
            .expect("worker must remain cancellable during backoff");
    }

    #[tokio::test]
    async fn disabled_worker_returns_immediately() {
        let (runner, _max) = FakeRunner::new(0, 0);
        let use_case = Arc::new(runner);
        let mut cfg = config();
        cfg.enabled = false;
        let handle = spawn_backfill_worker(use_case, cfg);
        // disabled Worker 不进入循环，shutdown 立即返回。
        tokio::time::timeout(Duration::from_secs(2), handle.shutdown())
            .await
            .expect("disabled worker shutdown must not hang");
    }

    #[test]
    fn wake_handle_does_not_block_without_consumers() {
        // Notify 可以在没有等待者时被通知而不阻塞或 panic。
        let notify = Arc::new(Notify::new());
        notify.notify_one();
        notify.notify_one();
    }

    #[allow(dead_code)]
    fn _metrics_marker() -> AtomicU32 {
        AtomicU32::new(0)
    }
    #[test]
    fn metrics_marker_compiles() {
        let _ = _metrics_marker().load(Ordering::Acquire);
    }
}
