//! 有界微批入站 Worker：从 NapCat WebSocket 回调的非阻塞队列中批量领取
//! 消息，在单个 MySQL 事务内持久化，并处理 poison 消息的二分隔离。
//!
//! 队列满时保持 Gap/Backfill 补偿语义。所有指标仅供内存使用，
//! 不改变持久化消息模型。

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use personal_secretary::{
    ArtifactEnvelope, ArtifactId, ArtifactKind, ArtifactUseCase, ConnectionEpochId, ContentSegment,
    ContentTrustLevel, InboundEventStoreError, InboundMessageEnvelope, IngestMessageOutcome,
    IngestionGapReason, MediaKind, PersonalSecretaryStoreT, RealtimeSpoolCheckpointPrefix,
    RealtimeSpoolFatal, RealtimeSpoolFatalKind, RealtimeSpoolHookKey, RealtimeSpoolReplayProgress,
    RecallCorrelationKey, RecallUseCase, RecoveredRealtimeSpoolFrame, RichContentKind,
    SourceEventId, checkpointable_prefix,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::config::IngestionConfig;

// ── 入队时间包装（仅内存使用，不改变持久化消息模型）──────────────────────

struct TimestampedEnvelope {
    envelope: InboundMessageEnvelope,
    enqueued_at: Instant,
    spool_frame: Option<RecoveredRealtimeSpoolFrame>,
}

// ── 溢出状态 ────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct OverflowState {
    dropped_count: AtomicU64,
    gap_persisted: AtomicBool,
}

impl OverflowState {
    fn record_drop(&self) -> u64 {
        // 每次新的溢出都需要重新持久化 Gap；旧 Gap 成功后不能抑制后续空窗。
        self.gap_persisted.store(false, Ordering::Release);
        self.dropped_count.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn dropped_count(&self) -> u64 {
        self.dropped_count.load(Ordering::Acquire)
    }

    fn gap_needs_persistence(&self) -> bool {
        self.dropped_count() > 0 && !self.gap_persisted.load(Ordering::Acquire)
    }

    fn mark_gap_persisted(&self) {
        self.gap_persisted.store(true, Ordering::Release);
    }
}

// ── 队列与入队 ──────────────────────────────────────────────────────────

pub struct IngestionQueue {
    sender: mpsc::Sender<TimestampedEnvelope>,
    overflow: Arc<OverflowState>,
    connection_epoch_id: ConnectionEpochId,
    /// 队列容量（固定值，用于计算 depth）。
    queue_capacity: usize,
    metrics: Arc<IngestionMetrics>,
}

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum IngestionEnqueueError {
    #[error("personal secretary ingestion queue is full")]
    Full,
    #[error("personal secretary ingestion queue is closed")]
    Closed,
}

impl IngestionQueue {
    /// 验收测试专用：构造一个有界队列但不启动 Worker。
    pub fn for_test() -> Self {
        let (sender, _receiver) = mpsc::channel(1);
        Self {
            sender,
            overflow: Arc::new(OverflowState::default()),
            connection_epoch_id: ConnectionEpochId::new("test-epoch").expect("valid id"),
            queue_capacity: 1,
            metrics: Arc::new(IngestionMetrics::default()),
        }
    }

    pub fn try_enqueue(
        &self,
        message: InboundMessageEnvelope,
    ) -> Result<(), IngestionEnqueueError> {
        let message = message.observed_in(self.connection_epoch_id.clone());
        let platform_message_id = message.source.message_id.clone();
        let conversation_id = message.conversation.id.clone();
        let enqueued_at = Instant::now();
        let wrapped = TimestampedEnvelope {
            envelope: message,
            enqueued_at,
            spool_frame: None,
        };
        match self.sender.try_send(wrapped) {
            Ok(()) => {
                self.metrics
                    .record_enqueue(self.queue_capacity, &self.sender);
                tracing::trace!(
                    connection_epoch_id = %self.connection_epoch_id.as_str(),
                    platform_message_id = %platform_message_id,
                    conversation_id = %conversation_id,
                    remaining_capacity = self.sender.capacity(),
                    "NapCat 消息已进入有界持久化队列"
                );
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                let dropped_count = self.overflow.record_drop();
                self.metrics.record_drop();
                tracing::warn!(
                    dropped_count,
                    "个人秘书持久化队列已满，消息未入队并将创建 uncertain Gap"
                );
                Err(IngestionEnqueueError::Full)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!("个人秘书持久化队列已关闭");
                Err(IngestionEnqueueError::Closed)
            }
        }
    }

    pub async fn enqueue_spooled(
        &self,
        frame: RecoveredRealtimeSpoolFrame,
    ) -> Result<(), IngestionEnqueueError> {
        let wrapped = TimestampedEnvelope {
            envelope: frame.message().clone(),
            enqueued_at: Instant::now(),
            spool_frame: Some(frame),
        };
        self.sender
            .send(wrapped)
            .await
            .map_err(|_| IngestionEnqueueError::Closed)?;
        self.metrics
            .record_enqueue(self.queue_capacity, &self.sender);
        Ok(())
    }

    pub fn blocking_enqueue_spooled(
        &self,
        frame: RecoveredRealtimeSpoolFrame,
    ) -> Result<(), IngestionEnqueueError> {
        let wrapped = TimestampedEnvelope {
            envelope: frame.message().clone(),
            enqueued_at: Instant::now(),
            spool_frame: Some(frame),
        };
        self.sender
            .blocking_send(wrapped)
            .map_err(|_| IngestionEnqueueError::Closed)?;
        self.metrics
            .record_enqueue(self.queue_capacity, &self.sender);
        Ok(())
    }
}

// ── 有界指标（仅内存，多线程安全）───────────────────────────────────────

#[derive(Debug, Default)]
pub struct IngestionMetrics {
    pub queue_capacity: AtomicU64,
    pub queue_depth: AtomicU64,
    pub in_flight: AtomicU64,
    pub high_watermark: AtomicU64,
    pub accepted: AtomicU64,
    pub duplicates: AtomicU64,
    pub invalid: AtomicU64,
    pub retries: AtomicU64,
    pub dropped: AtomicU64,
    pub batches_committed: AtomicU64,
    pub last_batch_size: AtomicU64,
    pub overflow_pending: AtomicU64,
    pub last_success_at: AtomicU64,
    pub last_failure_at: AtomicU64,
    pub enqueued: AtomicU64,
    pub committed: AtomicU64,
    pub commit_latency_count: AtomicU64,
    pub commit_latency_sum_ms: AtomicU64,
    pub commit_latency_max_ms: AtomicU64,
    pub last_commit_latency_ms: AtomicU64,
}

impl IngestionMetrics {
    fn record_enqueue(&self, queue_capacity: usize, sender: &mpsc::Sender<TimestampedEnvelope>) {
        saturating_increment(&self.enqueued);
        self.queue_capacity
            .store(queue_capacity as u64, Ordering::Release);
        let depth = queue_capacity.saturating_sub(sender.capacity());
        self.queue_depth.store(depth as u64, Ordering::Release);
        let prev = self.high_watermark.load(Ordering::Acquire);
        if (depth as u64) > prev {
            self.high_watermark.store(depth as u64, Ordering::Release);
        }
    }

    fn record_dequeue(&self, queue_capacity: usize, depth: usize) {
        self.queue_capacity
            .store(queue_capacity as u64, Ordering::Release);
        self.queue_depth
            .store(depth.min(queue_capacity) as u64, Ordering::Release);
    }

    fn record_drop(&self) {
        saturating_increment(&self.dropped);
        self.overflow_pending.store(1, Ordering::Release);
    }

    fn record_committed(&self, enqueued_at: impl IntoIterator<Item = Instant>) {
        for started in enqueued_at {
            let elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            saturating_increment(&self.committed);
            saturating_increment(&self.commit_latency_count);
            saturating_add(&self.commit_latency_sum_ms, elapsed_ms);
            self.commit_latency_max_ms
                .fetch_max(elapsed_ms, Ordering::AcqRel);
            self.last_commit_latency_ms
                .store(elapsed_ms, Ordering::Release);
        }
    }

    pub fn snapshot(&self) -> IngestionMetricSnapshot {
        IngestionMetricSnapshot {
            queue_capacity: self.queue_capacity.load(Ordering::Acquire),
            queue_depth: self.queue_depth.load(Ordering::Acquire),
            in_flight: self.in_flight.load(Ordering::Acquire),
            high_watermark: self.high_watermark.load(Ordering::Acquire),
            accepted: self.accepted.load(Ordering::Acquire),
            duplicates: self.duplicates.load(Ordering::Acquire),
            invalid: self.invalid.load(Ordering::Acquire),
            retries: self.retries.load(Ordering::Acquire),
            dropped: self.dropped.load(Ordering::Acquire),
            batches_committed: self.batches_committed.load(Ordering::Acquire),
            last_batch_size: self.last_batch_size.load(Ordering::Acquire),
            overflow_pending: self.overflow_pending.load(Ordering::Acquire),
            last_success_at: self.last_success_at.load(Ordering::Acquire),
            last_failure_at: self.last_failure_at.load(Ordering::Acquire),
            enqueued: self.enqueued.load(Ordering::Acquire),
            committed: self.committed.load(Ordering::Acquire),
            commit_latency_count: self.commit_latency_count.load(Ordering::Acquire),
            commit_latency_sum_ms: self.commit_latency_sum_ms.load(Ordering::Acquire),
            commit_latency_max_ms: self.commit_latency_max_ms.load(Ordering::Acquire),
            last_commit_latency_ms: self.last_commit_latency_ms.load(Ordering::Acquire),
        }
    }
}

fn saturating_increment(value: &AtomicU64) {
    saturating_add(value, 1);
}

fn saturating_add(value: &AtomicU64, amount: u64) {
    let _ = value.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_add(amount))
    });
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // 完整 public API 快照；部分字段仅 future health producer 使用
pub struct IngestionMetricSnapshot {
    pub queue_capacity: u64,
    pub queue_depth: u64,
    pub in_flight: u64,
    pub high_watermark: u64,
    pub accepted: u64,
    pub duplicates: u64,
    pub invalid: u64,
    pub retries: u64,
    pub dropped: u64,
    pub batches_committed: u64,
    pub last_batch_size: u64,
    pub overflow_pending: u64,
    pub last_success_at: u64,
    pub last_failure_at: u64,
    pub enqueued: u64,
    pub committed: u64,
    pub commit_latency_count: u64,
    pub commit_latency_sum_ms: u64,
    pub commit_latency_max_ms: u64,
    pub last_commit_latency_ms: u64,
}

// ── WorkerReport ─────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct WorkerReport {
    pub accepted: u64,
    pub duplicates: u64,
    pub invalid: u64,
    pub retries: u64,
    pub dropped: u64,
    pub batches_committed: u64,
}

/// 入站 Worker 向外报告健康状态的应用端口。
pub trait IngestionHealthReporterT: Send + Sync {
    fn mark_worker_success(&self, now_unix: u64);
    fn mark_worker_failure(&self);
}

#[async_trait::async_trait]
pub trait RealtimeSpoolCheckpointT: Send + Sync {
    async fn advance_checkpoint(
        &self,
        prefix: RealtimeSpoolCheckpointPrefix,
    ) -> Result<(), RealtimeSpoolFatal>;
}

// ── 启动入口 ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn spawn_ingestion_worker(
    store: Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: ConnectionEpochId,
    config: IngestionConfig,
    recall_use_case: Option<Arc<RecallUseCase>>,
    artifact_use_case: Option<Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
    health_reporter: Option<Arc<dyn IngestionHealthReporterT>>,
    external_metrics: Option<Arc<IngestionMetrics>>,
) -> (IngestionQueue, JoinHandle<WorkerReport>) {
    spawn_ingestion_worker_inner(
        store,
        connection_epoch_id,
        config,
        recall_use_case,
        artifact_use_case,
        artifact_default_ttl_secs,
        health_reporter,
        external_metrics,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_spooled_ingestion_worker(
    store: Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: ConnectionEpochId,
    config: IngestionConfig,
    recall_use_case: Option<Arc<RecallUseCase>>,
    artifact_use_case: Option<Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
    health_reporter: Option<Arc<dyn IngestionHealthReporterT>>,
    external_metrics: Option<Arc<IngestionMetrics>>,
    checkpoint: Arc<dyn RealtimeSpoolCheckpointT>,
    fatal_sender: mpsc::UnboundedSender<RealtimeSpoolFatal>,
) -> (IngestionQueue, JoinHandle<WorkerReport>) {
    spawn_ingestion_worker_inner(
        store,
        connection_epoch_id,
        config,
        recall_use_case,
        artifact_use_case,
        artifact_default_ttl_secs,
        health_reporter,
        external_metrics,
        Some(checkpoint),
        Some(fatal_sender),
    )
}

#[allow(clippy::too_many_arguments)]
fn spawn_ingestion_worker_inner(
    store: Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: ConnectionEpochId,
    config: IngestionConfig,
    recall_use_case: Option<Arc<RecallUseCase>>,
    artifact_use_case: Option<Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
    health_reporter: Option<Arc<dyn IngestionHealthReporterT>>,
    external_metrics: Option<Arc<IngestionMetrics>>,
    checkpoint: Option<Arc<dyn RealtimeSpoolCheckpointT>>,
    fatal_sender: Option<mpsc::UnboundedSender<RealtimeSpoolFatal>>,
) -> (IngestionQueue, JoinHandle<WorkerReport>) {
    let (sender, receiver) = mpsc::channel(config.queue_capacity);
    let overflow = Arc::new(OverflowState::default());
    let metrics = external_metrics.unwrap_or_default();
    let queue = IngestionQueue {
        sender,
        overflow: Arc::clone(&overflow),
        connection_epoch_id: connection_epoch_id.clone(),
        queue_capacity: config.queue_capacity,
        metrics: Arc::clone(&metrics),
    };
    let worker = tokio::spawn(run_worker(
        receiver,
        overflow,
        metrics,
        store,
        connection_epoch_id,
        config,
        recall_use_case,
        artifact_use_case,
        artifact_default_ttl_secs,
        health_reporter,
        checkpoint,
        fatal_sender,
    ));
    (queue, worker)
}

// ── Worker 主循环 ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_worker(
    mut receiver: mpsc::Receiver<TimestampedEnvelope>,
    overflow: Arc<OverflowState>,
    metrics: Arc<IngestionMetrics>,
    store: Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: ConnectionEpochId,
    config: IngestionConfig,
    recall_use_case: Option<Arc<RecallUseCase>>,
    artifact_use_case: Option<Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
    health_reporter: Option<Arc<dyn IngestionHealthReporterT>>,
    checkpoint: Option<Arc<dyn RealtimeSpoolCheckpointT>>,
    fatal_sender: Option<mpsc::UnboundedSender<RealtimeSpoolFatal>>,
) -> WorkerReport {
    tracing::debug!(
        connection_epoch_id = %connection_epoch_id.as_str(),
        queue_capacity = config.queue_capacity,
        batch_size = config.batch_size,
        batch_flush_ms = config.batch_flush_ms,
        retry_initial_ms = config.retry_initial_ms,
        retry_max_ms = config.retry_max_ms,
        "个人秘书微批持久化 Worker 已启动"
    );

    let flush_timeout = Duration::from_millis(config.batch_flush_ms);
    let batch_capacity = config.batch_size;
    let mut fatal_observed = false;

    loop {
        // 批次开始前持久化溢出 Gap。
        persist_overflow_gap(&store, &connection_epoch_id, &overflow, &metrics).await;

        // 等待第一条消息（channel 关闭时退出）。
        let first = match receiver.recv().await {
            Some(msg) => msg,
            None => break,
        };
        let oldest_enqueued_age = Some(first.enqueued_at);
        let mut batch: Vec<TimestampedEnvelope> = Vec::with_capacity(batch_capacity);
        batch.push(first);

        // 用 try_recv 快速填满批次。
        fill_batch(&mut receiver, &mut batch, batch_capacity);
        metrics.record_dequeue(config.queue_capacity, receiver.len());

        // 未满且还有剩余容量时，最多等待 flush_timeout。
        if batch.len() < batch_capacity {
            let flush_deadline = Instant::now() + flush_timeout;
            loop {
                let remaining = flush_deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() || batch.len() >= batch_capacity {
                    break;
                }
                match tokio::time::timeout(remaining, receiver.recv()).await {
                    Ok(Some(msg)) => {
                        batch.push(msg);
                        fill_batch(&mut receiver, &mut batch, batch_capacity);
                        metrics.record_dequeue(config.queue_capacity, receiver.len());
                    }
                    Ok(None) => break, // channel 关闭
                    Err(_) => break,   // 超时
                }
            }
        }

        if batch.is_empty() {
            continue;
        }

        metrics
            .in_flight
            .store(batch.len() as u64, Ordering::Release);

        // 实际处理批次（含 retry、poison 二分隔离）。
        let result = process_batch_with_retry(
            &store,
            &connection_epoch_id,
            &overflow,
            &config,
            &batch,
            &metrics,
            oldest_enqueued_age,
            recall_use_case.as_ref(),
            artifact_use_case.as_ref(),
            artifact_default_ttl_secs,
            health_reporter.as_ref(),
            checkpoint.as_ref(),
        )
        .await;
        if let Err(fatal) = result {
            if let Some(sender) = &fatal_sender {
                let _ = sender.send(fatal);
            }
            fatal_observed = true;
            break;
        }

        metrics.record_dequeue(config.queue_capacity, receiver.len());
        metrics.in_flight.store(0, Ordering::Release);
    }

    // 排空：channel 关闭后处理剩余消息。
    let mut drain: Vec<TimestampedEnvelope> = Vec::with_capacity(batch_capacity);
    while !fatal_observed && let Ok(msg) = receiver.try_recv() {
        drain.push(msg);
        if drain.len() >= batch_capacity {
            metrics
                .in_flight
                .store(drain.len() as u64, Ordering::Release);
            if let Err(fatal) = process_batch_with_retry(
                &store,
                &connection_epoch_id,
                &overflow,
                &config,
                &drain,
                &metrics,
                None,
                recall_use_case.as_ref(),
                artifact_use_case.as_ref(),
                artifact_default_ttl_secs,
                health_reporter.as_ref(),
                checkpoint.as_ref(),
            )
            .await
            {
                if let Some(sender) = &fatal_sender {
                    let _ = sender.send(fatal);
                }
                fatal_observed = true;
                break;
            }
            drain.clear();
            metrics.record_dequeue(config.queue_capacity, receiver.len());
            metrics.in_flight.store(0, Ordering::Release);
        }
    }
    if !fatal_observed && !drain.is_empty() {
        metrics
            .in_flight
            .store(drain.len() as u64, Ordering::Release);
        let result = process_batch_with_retry(
            &store,
            &connection_epoch_id,
            &overflow,
            &config,
            &drain,
            &metrics,
            None,
            recall_use_case.as_ref(),
            artifact_use_case.as_ref(),
            artifact_default_ttl_secs,
            health_reporter.as_ref(),
            checkpoint.as_ref(),
        )
        .await;
        if let (Err(fatal), Some(sender)) = (result, &fatal_sender) {
            let _ = sender.send(fatal);
        }
        metrics.record_dequeue(config.queue_capacity, receiver.len());
        metrics.in_flight.store(0, Ordering::Release);
    }

    persist_overflow_gap(&store, &connection_epoch_id, &overflow, &metrics).await;

    let snapshot = metrics.snapshot();
    let report = WorkerReport {
        accepted: snapshot.accepted,
        duplicates: snapshot.duplicates,
        invalid: snapshot.invalid,
        retries: snapshot.retries,
        dropped: snapshot.dropped,
        batches_committed: snapshot.batches_committed,
    };
    tracing::debug!(
        connection_epoch_id = %connection_epoch_id.as_str(),
        accepted = report.accepted,
        duplicates = report.duplicates,
        invalid = report.invalid,
        retries = report.retries,
        dropped = report.dropped,
        batches_committed = report.batches_committed,
        "个人秘书微批持久化 Worker 已排空并退出"
    );
    report
}

/// 从 channel 快速填充批次（非阻塞）。
fn fill_batch(
    receiver: &mut mpsc::Receiver<TimestampedEnvelope>,
    batch: &mut Vec<TimestampedEnvelope>,
    batch_capacity: usize,
) {
    while batch.len() < batch_capacity {
        match receiver.try_recv() {
            Ok(msg) => batch.push(msg),
            Err(_) => break,
        }
    }
}

// ── 批次处理（重试 + poison 二分隔离）───────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn process_batch_with_retry(
    store: &Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: &ConnectionEpochId,
    overflow: &Arc<OverflowState>,
    config: &IngestionConfig,
    batch: &[TimestampedEnvelope],
    metrics: &Arc<IngestionMetrics>,
    _oldest_enqueued_age: Option<Instant>,
    recall_use_case: Option<&Arc<RecallUseCase>>,
    artifact_use_case: Option<&Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
    health_reporter: Option<&Arc<dyn IngestionHealthReporterT>>,
    checkpoint: Option<&Arc<dyn RealtimeSpoolCheckpointT>>,
) -> Result<(), RealtimeSpoolFatal> {
    if batch.iter().any(|item| item.spool_frame.is_some()) {
        if batch.iter().any(|item| item.spool_frame.is_none()) || checkpoint.is_none() {
            return Err(RealtimeSpoolFatal::new(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            ));
        }
        return process_spooled_batch_with_retry(
            store,
            config,
            batch,
            metrics,
            recall_use_case,
            artifact_use_case,
            artifact_default_ttl_secs,
            health_reporter,
            checkpoint.expect("checked above"),
        )
        .await;
    }
    // 将整批推入二分队列（有界迭代，绝不递归爆栈）。
    let mut queue: VecDeque<Vec<usize>> = VecDeque::new();
    queue.push_back((0..batch.len()).collect());

    let mut retry_delay = Duration::from_millis(config.retry_initial_ms);
    let max_retry_delay = Duration::from_millis(config.retry_max_ms);

    while let Some(mut current_indexes) = queue.pop_front() {
        if current_indexes.is_empty() {
            continue;
        }
        let current_batch = current_indexes
            .iter()
            .map(|index| batch[*index].envelope.clone())
            .collect::<Vec<_>>();
        persist_overflow_gap(store, connection_epoch_id, overflow, metrics).await;

        let mut attempt = 0_u64;
        loop {
            attempt += 1;
            match store.insert_messages_if_absent(&current_batch).await {
                Ok(outcomes) => {
                    // 事务成功提交：处理 post-hooks（recall、artifact）。
                    metrics.accepted.fetch_add(
                        outcomes
                            .iter()
                            .filter(|o| matches!(o, IngestMessageOutcome::Accepted { .. }))
                            .count() as u64,
                        Ordering::AcqRel,
                    );
                    metrics.duplicates.fetch_add(
                        outcomes
                            .iter()
                            .filter(|o| matches!(o, IngestMessageOutcome::Duplicate { .. }))
                            .count() as u64,
                        Ordering::AcqRel,
                    );
                    metrics.batches_committed.fetch_add(1, Ordering::AcqRel);
                    metrics
                        .last_batch_size
                        .store(current_batch.len() as u64, Ordering::Release);
                    metrics
                        .last_success_at
                        .store(current_unix_secs(), Ordering::Release);
                    metrics.record_committed(
                        current_indexes
                            .iter()
                            .map(|index| batch[*index].enqueued_at),
                    );

                    if let Some(health) = health_reporter {
                        health.mark_worker_success(current_unix_secs());
                    }

                    // Post-hooks 必须在事务成功后执行。
                    for (i, outcome) in outcomes.iter().enumerate() {
                        let _ = fire_post_hooks(
                            outcome,
                            &current_batch[i],
                            recall_use_case,
                            artifact_use_case,
                            artifact_default_ttl_secs,
                        )
                        .await;
                    }
                    if !queue.is_empty() {
                        tracing::debug!(
                            sub_batch_len = current_batch.len(),
                            remaining_splits = queue.len(),
                            "poison 二分隔离子批次已提交"
                        );
                    }
                    break; // 当前子批次成功，继续处理队列中下一子批次
                }
                Err(InboundEventStoreError::InvalidData(_error)) => {
                    if current_batch.len() == 1 {
                        // 单条 poison：标记 invalid，跳过。
                        metrics.invalid.fetch_add(1, Ordering::AcqRel);
                        metrics
                            .last_failure_at
                            .store(current_unix_secs(), Ordering::Release);
                        if let Some(health) = health_reporter {
                            health.mark_worker_failure();
                        }
                        if let Err(gap_error) = store
                            .mark_connection_uncertain(
                                connection_epoch_id,
                                IngestionGapReason::InvalidEvent,
                            )
                            .await
                        {
                            tracing::error!(
                                error_code = inbound_error_code(&gap_error),
                                "无法为非法队列消息持久化 uncertain Gap"
                            );
                        }
                        tracing::warn!("单条非法消息已隔离并跳过，创建 InvalidEvent uncertain Gap");
                        break; // 此子批次处理完成（跳过）
                    }
                    // 长度 > 1：事务已回滚，二分继续定位。
                    let mid = current_indexes.len() / 2;
                    let right = current_indexes.split_off(mid);
                    // 按原顺序：先左半后右半（保持入队顺序）。
                    queue.push_front(right);
                    queue.push_front(current_indexes);
                    tracing::debug!(
                        batch_len_left = mid,
                        batch_len_right = queue.front().map(|b| b.len()).unwrap_or(0),
                        "批次含 poison 消息，已二分为子批次继续定位"
                    );
                    break; // 跳出 retry loop，处理子批次
                }
                Err(error) => {
                    // 暂时性错误（Database/Unavailable）：整体重试。
                    metrics.retries.fetch_add(1, Ordering::AcqRel);
                    metrics
                        .last_failure_at
                        .store(current_unix_secs(), Ordering::Release);
                    if let Some(health) = health_reporter {
                        health.mark_worker_failure();
                    }
                    if attempt == 1 || attempt.is_power_of_two() {
                        tracing::warn!(
                            batch_len = current_batch.len(),
                            attempt,
                            retry_delay_ms = retry_delay.as_millis(),
                            error_code = inbound_error_code(&error),
                            "批次持久化失败，将整体重试"
                        );
                    }
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = retry_delay.saturating_mul(2).min(max_retry_delay);
                    // 循环继续重试同一批次。
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_spooled_batch_with_retry(
    store: &Arc<dyn PersonalSecretaryStoreT>,
    config: &IngestionConfig,
    batch: &[TimestampedEnvelope],
    metrics: &Arc<IngestionMetrics>,
    recall_use_case: Option<&Arc<RecallUseCase>>,
    artifact_use_case: Option<&Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
    health_reporter: Option<&Arc<dyn IngestionHealthReporterT>>,
    checkpoint: &Arc<dyn RealtimeSpoolCheckpointT>,
) -> Result<(), RealtimeSpoolFatal> {
    let messages = batch
        .iter()
        .map(|item| item.envelope.clone())
        .collect::<Vec<_>>();
    let mut retry_delay = Duration::from_millis(config.retry_initial_ms);
    let max_retry_delay = Duration::from_millis(config.retry_max_ms);
    let outcomes = loop {
        match store.insert_messages_if_absent(&messages).await {
            Ok(outcomes) => break outcomes,
            Err(InboundEventStoreError::InvalidData(_)) => {
                metrics.invalid.fetch_add(1, Ordering::AcqRel);
                metrics
                    .last_failure_at
                    .store(current_unix_secs(), Ordering::Release);
                return Err(RealtimeSpoolFatal::new(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                ));
            }
            Err(error) => {
                metrics.retries.fetch_add(1, Ordering::AcqRel);
                metrics
                    .last_failure_at
                    .store(current_unix_secs(), Ordering::Release);
                if let Some(health) = health_reporter {
                    health.mark_worker_failure();
                }
                tracing::warn!(
                    error_code = inbound_error_code(&error),
                    retry_delay_ms = retry_delay.as_millis(),
                    "durable 消息批次持久化失败，将保持 WAL 并重试"
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay.saturating_mul(2).min(max_retry_delay);
            }
        }
    };
    metrics.record_committed(batch.iter().map(|item| item.enqueued_at));

    let mut progress = Vec::with_capacity(batch.len());
    for (item, outcome) in batch.iter().zip(&outcomes) {
        let required_hooks = required_hook_keys(
            &item.envelope,
            outcome,
            recall_use_case.is_some(),
            artifact_use_case.is_some(),
        )?;
        let mut hook_delay = Duration::from_millis(config.retry_initial_ms);
        loop {
            match fire_post_hooks(
                outcome,
                &item.envelope,
                recall_use_case,
                artifact_use_case,
                artifact_default_ttl_secs,
            )
            .await
            {
                Ok(()) => break,
                Err(()) => {
                    metrics.retries.fetch_add(1, Ordering::AcqRel);
                    metrics
                        .last_failure_at
                        .store(current_unix_secs(), Ordering::Release);
                    if let Some(health) = health_reporter {
                        health.mark_worker_failure();
                    }
                    tracing::warn!(
                        retry_delay_ms = hook_delay.as_millis(),
                        error_code = "required_hook_not_converged",
                        "durable 消息必需 hook 尚未收敛，将保持 WAL 并重试"
                    );
                    tokio::time::sleep(hook_delay).await;
                    hook_delay = hook_delay.saturating_mul(2).min(max_retry_delay);
                }
            }
        }
        let frame = item.spool_frame.clone().ok_or_else(|| {
            RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::RecoveryInvariantViolation)
        })?;
        let mut entry = RealtimeSpoolReplayProgress::pending(frame, required_hooks.clone())
            .with_ingestion(outcome.clone());
        for hook in required_hooks {
            entry = entry.with_converged_hook(hook);
        }
        progress.push(entry);
    }

    let generation = progress
        .first()
        .map(|entry| entry.frame().generation_id().clone())
        .ok_or_else(|| {
            RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::RecoveryInvariantViolation)
        })?;
    let prefix = checkpointable_prefix(generation, &progress);
    checkpoint.advance_checkpoint(prefix).await?;

    metrics.accepted.fetch_add(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, IngestMessageOutcome::Accepted { .. }))
            .count() as u64,
        Ordering::AcqRel,
    );
    metrics.duplicates.fetch_add(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, IngestMessageOutcome::Duplicate { .. }))
            .count() as u64,
        Ordering::AcqRel,
    );
    metrics.batches_committed.fetch_add(1, Ordering::AcqRel);
    metrics
        .last_batch_size
        .store(batch.len() as u64, Ordering::Release);
    metrics
        .last_success_at
        .store(current_unix_secs(), Ordering::Release);
    if let Some(health) = health_reporter {
        health.mark_worker_success(current_unix_secs());
    }
    Ok(())
}

// ── Post-hooks ───────────────────────────────────────────────────────────

pub(crate) async fn fire_post_hooks(
    outcome: &IngestMessageOutcome,
    message: &InboundMessageEnvelope,
    recall_use_case: Option<&Arc<RecallUseCase>>,
    artifact_use_case: Option<&Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
) -> Result<(), ()> {
    let source_event_id = outcome.source_event_id();
    maybe_apply_pending_recall(recall_use_case, message, source_event_id.as_str()).await?;
    maybe_create_artifacts(
        artifact_use_case,
        message,
        source_event_id.as_str(),
        artifact_default_ttl_secs,
    )
    .await?;
    Ok(())
}

pub(crate) fn required_hook_keys(
    message: &InboundMessageEnvelope,
    outcome: &IngestMessageOutcome,
    recall_enabled: bool,
    artifact_enabled: bool,
) -> Result<Vec<RealtimeSpoolHookKey>, RealtimeSpoolFatal> {
    let mut hooks = Vec::new();
    if recall_enabled {
        let correlation = RecallCorrelationKey::new(
            message.source.account_ref(),
            message.source.channel,
            message.conversation.clone(),
            message.source.message_id.clone(),
        )
        .map_err(|_| RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::RecoveryInvariantViolation))?;
        hooks.push(RealtimeSpoolHookKey::recall(correlation));
    }
    if artifact_enabled {
        let source_event_id =
            SourceEventId::new(outcome.source_event_id().as_str()).map_err(|_| {
                RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::RecoveryInvariantViolation)
            })?;
        for (ordinal, segment) in message.segments.iter().enumerate() {
            if let Some(kind) = artifact_kind(segment) {
                hooks.push(RealtimeSpoolHookKey::artifact(
                    ArtifactId::for_source_segment(&source_event_id, ordinal, kind),
                ));
            }
        }
    }
    Ok(hooks)
}

fn artifact_kind(segment: &ContentSegment) -> Option<ArtifactKind> {
    match segment {
        ContentSegment::Media { kind, .. } => Some(match kind {
            MediaKind::Image => ArtifactKind::Image,
            MediaKind::Audio => ArtifactKind::Record,
            MediaKind::Video => ArtifactKind::Video,
            MediaKind::File => ArtifactKind::File,
        }),
        ContentSegment::Forward { .. } => Some(ArtifactKind::Forward),
        ContentSegment::Rich { kind, .. } => Some(match kind {
            RichContentKind::Json => ArtifactKind::RichJson,
            RichContentKind::Xml => ArtifactKind::RichXml,
            RichContentKind::Card => ArtifactKind::RichCard,
        }),
        _ => None,
    }
}

// ── Artifact / Recall 辅助（从旧实现迁移，逻辑不变）────────────────────

async fn maybe_create_artifacts(
    artifact_use_case: Option<&Arc<ArtifactUseCase>>,
    message: &InboundMessageEnvelope,
    source_event_id: &str,
    default_ttl_secs: u64,
) -> Result<(), ()> {
    let Some(use_case) = artifact_use_case else {
        return Ok(());
    };
    let source_event_id = SourceEventId::new(source_event_id).map_err(|_| ())?;
    let ttl = if default_ttl_secs == 0 {
        None
    } else {
        Some(
            message
                .occurred_at_unix_secs
                .saturating_add(default_ttl_secs as i64),
        )
    };
    for (segment_ordinal, segment) in message.segments.iter().enumerate() {
        let (artifact_kind, source_key, display_name, description) = match segment {
            ContentSegment::Media {
                kind,
                source_key,
                display_name,
                ..
            } => (
                match kind {
                    MediaKind::Image => ArtifactKind::Image,
                    MediaKind::Audio => ArtifactKind::Record,
                    MediaKind::Video => ArtifactKind::Video,
                    MediaKind::File => ArtifactKind::File,
                },
                source_key,
                display_name.clone(),
                None,
            ),
            ContentSegment::Forward { source_key } => {
                (ArtifactKind::Forward, source_key, None, None)
            }
            ContentSegment::Rich {
                kind,
                source_key,
                summary,
            } => (
                match kind {
                    RichContentKind::Json => ArtifactKind::RichJson,
                    RichContentKind::Xml => ArtifactKind::RichXml,
                    RichContentKind::Card => ArtifactKind::RichCard,
                },
                source_key,
                None,
                summary.clone(),
            ),
            _ => continue,
        };
        let artifact_id =
            ArtifactId::for_source_segment(&source_event_id, segment_ordinal, artifact_kind);
        let mut envelope = match ArtifactEnvelope::new(
            artifact_id,
            message.source.account_ref(),
            source_event_id.clone(),
            message.conversation.clone(),
            artifact_kind,
            source_key.clone(),
            ContentTrustLevel::Normal,
            message.occurred_at_unix_secs,
            ttl,
        ) {
            Ok(env) => env,
            Err(error) => {
                let _ = error;
                return Err(());
            }
        };
        if let Some(name) = display_name {
            envelope = envelope.with_display_name(Some(name));
        }
        if let Some(description) = description {
            envelope = envelope.with_description(Some(description));
        }
        if let Err(error) = use_case.create(&envelope).await {
            let _ = error;
            return Err(());
        }
    }
    Ok(())
}

async fn maybe_apply_pending_recall(
    recall_use_case: Option<&Arc<RecallUseCase>>,
    message: &InboundMessageEnvelope,
    source_event_id: &str,
) -> Result<(), ()> {
    let Some(use_case) = recall_use_case else {
        return Ok(());
    };
    let correlation = match RecallCorrelationKey::new(
        message.source.account_ref(),
        message.source.channel,
        message.conversation.clone(),
        message.source.message_id.clone(),
    ) {
        Ok(key) => key,
        Err(_) => return Err(()),
    };
    match use_case
        .on_message_ingested(&correlation, source_event_id)
        .await
    {
        Ok(Some(_record)) => {}
        Ok(None) => {}
        Err(error) => {
            let _ = error;
            return Err(());
        }
    }
    Ok(())
}

// ── 辅助函数 ─────────────────────────────────────────────────────────────

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn persist_overflow_gap(
    store: &Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: &ConnectionEpochId,
    overflow: &OverflowState,
    metrics: &IngestionMetrics,
) {
    if !overflow.gap_needs_persistence() {
        return;
    }
    let dropped_count = overflow.dropped_count();
    match store
        .mark_connection_uncertain(connection_epoch_id, IngestionGapReason::QueueOverflow)
        .await
    {
        Ok(_) => {
            overflow.mark_gap_persisted();
            metrics.overflow_pending.store(
                u64::from(overflow.gap_needs_persistence()),
                Ordering::Release,
            );
            tracing::warn!(dropped_count, "队列溢出已持久化为 uncertain Gap");
        }
        Err(error) => {
            let _ = error;
            tracing::debug!(
            connection_epoch_id = %connection_epoch_id.as_str(),
            dropped_count,
            error_code = "overflow_gap_persist_failed",
            "队列溢出 Gap 暂未持久化，将随 Worker 重试"
            )
        }
    }
}

fn inbound_error_code(error: &InboundEventStoreError) -> &'static str {
    match error {
        InboundEventStoreError::InvalidData(_) => "invalid_data",
        InboundEventStoreError::Unavailable => "unavailable",
        InboundEventStoreError::Database(_) => "database_error",
        InboundEventStoreError::LeaseLost => "lease_lost",
    }
}

// ── 测试 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU64;

    use async_trait::async_trait;
    use personal_secretary::{
        ClaimedRecallEvent, ConnectionEndReason, ConversationKind, ConversationRef,
        InboundEventStoreT, IngestionContinuityStoreT, IngestionGapId, MessageSource,
        RealtimeSpoolCheckpointPrefix, RealtimeSpoolGenerationId, RealtimeSpoolRecordId,
        RecallCorrelationKey, RecallEvent, RecallFailureKind, RecallStoreError, RecallStoreT,
        RecoveredRealtimeSpoolFrame, SourceAccountRef, SourceEventId, SourceMessageRef,
        TombstoneRecord, TombstoneStatus, VerifiedActor, VerifiedActorKind,
    };

    use super::*;

    fn message(id: &str) -> InboundMessageEnvelope {
        InboundMessageEnvelope::new(
            SourceMessageRef::new(MessageSource::NapCat, "account-1", id).unwrap(),
            ConversationRef::new(ConversationKind::Group, "group-1").unwrap(),
            VerifiedActor::new(VerifiedActorKind::External, "actor-1").unwrap(),
            100,
            "",
            Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn full_queue_rejects_immediately_and_counts_the_gap() {
        let (sender, _receiver) = mpsc::channel(1);
        let overflow = Arc::new(OverflowState::default());
        let metrics = Arc::new(IngestionMetrics::default());
        let queue = IngestionQueue {
            sender,
            overflow: Arc::clone(&overflow),
            connection_epoch_id: ConnectionEpochId::new("epoch-1").unwrap(),
            queue_capacity: 1,
            metrics,
        };

        queue.try_enqueue(message("message-1")).unwrap();
        let error = queue.try_enqueue(message("message-2")).unwrap_err();

        assert_eq!(error, IngestionEnqueueError::Full);
        assert_eq!(overflow.dropped_count(), 1);
        assert!(overflow.gap_needs_persistence());
    }

    #[test]
    fn retry_delay_doubles_but_is_capped() {
        let mut delay = Duration::from_millis(100);
        let max = Duration::from_millis(250);
        delay = delay.saturating_mul(2).min(max);
        assert_eq!(delay, Duration::from_millis(200));
        delay = delay.saturating_mul(2).min(max);
        assert_eq!(delay, max);
    }

    /// Fake store：返回 results 列表中的结果，支持 insert_messages_if_absent。
    struct BatchStore {
        results: Mutex<VecDeque<Vec<IngestMessageOutcome>>>,
        errors: Mutex<VecDeque<InboundEventStoreError>>,
        insert_attempts: AtomicU64,
        max_batch_size: AtomicU64,
    }

    #[async_trait]
    impl InboundEventStoreT for BatchStore {
        async fn insert_message_if_absent(
            &self,
            _message: &InboundMessageEnvelope,
        ) -> Result<IngestMessageOutcome, InboundEventStoreError> {
            unimplemented!("batch tests use insert_messages_if_absent")
        }

        async fn insert_messages_if_absent(
            &self,
            messages: &[InboundMessageEnvelope],
        ) -> Result<Vec<IngestMessageOutcome>, InboundEventStoreError> {
            self.insert_attempts.fetch_add(1, Ordering::AcqRel);
            self.max_batch_size
                .fetch_max(messages.len() as u64, Ordering::AcqRel);
            if let Some(error) = self.errors.lock().unwrap().pop_front() {
                return Err(error);
            }
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(InboundEventStoreError::Unavailable)
                .inspect(|outcomes| {
                    assert_eq!(
                        outcomes.len(),
                        messages.len(),
                        "fake batch store: outcomes.len() must match batch len"
                    );
                })
        }
    }

    #[async_trait]
    impl IngestionContinuityStoreT for BatchStore {
        async fn begin_connection(
            &self,
            _account: &SourceAccountRef,
        ) -> Result<ConnectionEpochId, InboundEventStoreError> {
            ConnectionEpochId::new("epoch-batch")
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))
        }

        async fn mark_connection_connected(
            &self,
            _connection_epoch_id: &ConnectionEpochId,
        ) -> Result<(), InboundEventStoreError> {
            Ok(())
        }

        async fn finish_connection(
            &self,
            _connection_epoch_id: &ConnectionEpochId,
            _reason: ConnectionEndReason,
        ) -> Result<Option<IngestionGapId>, InboundEventStoreError> {
            Ok(None)
        }

        async fn mark_connection_uncertain(
            &self,
            _connection_epoch_id: &ConnectionEpochId,
            _reason: IngestionGapReason,
        ) -> Result<IngestionGapId, InboundEventStoreError> {
            IngestionGapId::new("gap-batch")
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))
        }
    }

    fn accepted(id: &str) -> IngestMessageOutcome {
        IngestMessageOutcome::Accepted {
            source_event_id: SourceEventId::new(id).unwrap(),
            reply_to_event_id: None,
        }
    }

    fn duplicate(id: &str) -> IngestMessageOutcome {
        IngestMessageOutcome::Duplicate {
            source_event_id: SourceEventId::new(id).unwrap(),
        }
    }

    #[derive(Default)]
    struct CountingCheckpoint {
        advances: AtomicU64,
    }

    #[async_trait]
    impl RealtimeSpoolCheckpointT for CountingCheckpoint {
        async fn advance_checkpoint(
            &self,
            _prefix: RealtimeSpoolCheckpointPrefix,
        ) -> Result<(), RealtimeSpoolFatal> {
            self.advances.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FailingRecallStore {
        calls: AtomicU64,
    }

    #[async_trait]
    impl RecallStoreT for FailingRecallStore {
        async fn record_recall(
            &self,
            _recall: &RecallEvent,
        ) -> Result<TombstoneStatus, RecallStoreError> {
            unreachable!("not used by this test")
        }

        async fn apply_pending_tombstone(
            &self,
            _correlation: &RecallCorrelationKey,
            _source_event_id: &str,
        ) -> Result<Option<TombstoneRecord>, RecallStoreError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Err(RecallStoreError::Unavailable("injected".into()))
        }

        async fn list_pending_for_correlation(
            &self,
            _correlation: &RecallCorrelationKey,
        ) -> Result<Vec<TombstoneRecord>, RecallStoreError> {
            unreachable!("not used by this test")
        }

        async fn is_recalled(
            &self,
            _account_id: u64,
            _source_event_id: &str,
        ) -> Result<bool, RecallStoreError> {
            unreachable!("not used by this test")
        }

        async fn list_recalled_event_ids(
            &self,
            _account_id: u64,
        ) -> Result<Vec<String>, RecallStoreError> {
            unreachable!("not used by this test")
        }

        async fn enqueue_recall(&self, _recall: &RecallEvent) -> Result<(), RecallStoreError> {
            unreachable!("not used by this test")
        }

        async fn claim_recall(
            &self,
            _lease_secs: u64,
        ) -> Result<Option<ClaimedRecallEvent>, RecallStoreError> {
            unreachable!("not used by this test")
        }

        async fn mark_recall_applied(
            &self,
            _recall_event_id: &str,
            _lease_token: &str,
        ) -> Result<(), RecallStoreError> {
            unreachable!("not used by this test")
        }

        async fn mark_recall_failed(
            &self,
            _recall_event_id: &str,
            _lease_token: &str,
            _error_code: &str,
            _kind: RecallFailureKind,
        ) -> Result<(), RecallStoreError> {
            unreachable!("not used by this test")
        }
    }

    #[tokio::test]
    async fn required_hook_failure_never_advances_spool_checkpoint() {
        let store = Arc::new(BatchStore {
            results: Mutex::new(VecDeque::from([vec![accepted("evt-hook")]])),
            errors: Mutex::new(VecDeque::new()),
            insert_attempts: AtomicU64::new(0),
            max_batch_size: AtomicU64::new(0),
        });
        let recall_store = Arc::new(FailingRecallStore::default());
        let recall_port: Arc<dyn RecallStoreT> = recall_store.clone();
        let recall = Arc::new(RecallUseCase::new(recall_port));
        let checkpoint = Arc::new(CountingCheckpoint::default());
        let checkpoint_port: Arc<dyn RealtimeSpoolCheckpointT> = checkpoint.clone();
        let epoch = ConnectionEpochId::new("epoch-hook").unwrap();
        let config = IngestionConfig {
            queue_capacity: 2,
            batch_size: 1,
            batch_flush_ms: 1,
            retry_initial_ms: 1,
            retry_max_ms: 2,
            shutdown_drain_timeout_secs: 1,
        };
        let store_port: Arc<dyn PersonalSecretaryStoreT> = store;
        let (queue, worker) = spawn_spooled_ingestion_worker(
            store_port,
            epoch.clone(),
            config,
            Some(recall),
            None,
            0,
            None,
            None,
            checkpoint_port,
            mpsc::unbounded_channel().0,
        );
        let observed = message("message-hook").observed_in(epoch.clone());
        let frame = RecoveredRealtimeSpoolFrame::new(
            RealtimeSpoolGenerationId::new("generation-hook").unwrap(),
            RealtimeSpoolRecordId::new("record-hook").unwrap(),
            epoch,
            observed,
        )
        .unwrap();

        queue.enqueue_spooled(frame).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while recall_store.calls.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(recall_store.calls.load(Ordering::Acquire) > 0);
        assert_eq!(checkpoint.advances.load(Ordering::Acquire), 0);
        worker.abort();
        let _ = worker.await;
    }

    #[tokio::test]
    async fn database_failure_retries_in_worker_and_recovers_without_callback_blocking() {
        let store = Arc::new(BatchStore {
            results: Mutex::new(VecDeque::from(vec![vec![accepted("evt-msg-retry")]])),
            errors: Mutex::new(VecDeque::from(vec![
                InboundEventStoreError::Unavailable,
                InboundEventStoreError::Unavailable,
            ])),
            insert_attempts: AtomicU64::new(0),
            max_batch_size: AtomicU64::new(0),
        });
        let config = IngestionConfig {
            queue_capacity: 1,
            batch_size: 64,
            batch_flush_ms: 5,
            retry_initial_ms: 1,
            retry_max_ms: 2,
            shutdown_drain_timeout_secs: 1,
        };
        let store_port: Arc<dyn PersonalSecretaryStoreT> = store.clone();
        let (queue, worker) = spawn_ingestion_worker(
            store_port,
            ConnectionEpochId::new("epoch-retry").unwrap(),
            config,
            None,
            None,
            0,
            None,
            None,
        );

        queue.try_enqueue(message("message-retry")).unwrap();
        drop(queue);
        let report = worker.await.unwrap();

        assert_eq!(report.accepted, 1);
        assert_eq!(report.retries, 2);
        assert_eq!(store.insert_attempts.load(Ordering::Acquire), 3);
    }

    #[tokio::test]
    async fn poison_message_binary_split_isolates_and_skips_without_losing_neighbors() {
        // 3 条消息：中间那条是 poison（InvalidData），左右邻居应全部提交。
        // 第一次调用 insert_messages_if_absent([msg-ok-1, msg-poison, msg-ok-2]) → InvalidData
        // 二分：左 [msg-ok-1]，中 [msg-poison, msg-ok-2]
        // 左 [msg-ok-1] → Ok → post-hooks
        // 中 [msg-poison, msg-ok-2] → InvalidData → 二分
        //   → 左 [msg-poison] → InvalidData + len==1 → invalid 计数 + skip
        //   → 右 [msg-ok-2] → Ok → post-hooks
        let store = Arc::new(BatchStore {
            results: Mutex::new(VecDeque::from(vec![
                // [msg-ok-1] 子批次 → 成功
                vec![accepted("evt-ok-1")],
                // [msg-ok-2] 子批次 → 成功
                vec![accepted("evt-ok-2")],
            ])),
            errors: Mutex::new(VecDeque::from(vec![
                // 整批 3 条 → InvalidData
                InboundEventStoreError::InvalidData("poison_in_batch".into()),
                // [msg-poison, msg-ok-2] → InvalidData（再二分）
                InboundEventStoreError::InvalidData("poison_still_present".into()),
                // [msg-poison] (len==1) → InvalidData → worker 标记 invalid 跳过
                InboundEventStoreError::InvalidData("poison_isolated".into()),
            ])),
            insert_attempts: AtomicU64::new(0),
            max_batch_size: AtomicU64::new(0),
        });
        let config = IngestionConfig {
            queue_capacity: 4,
            batch_size: 64,
            batch_flush_ms: 5,
            retry_initial_ms: 1,
            retry_max_ms: 2,
            shutdown_drain_timeout_secs: 1,
        };
        let store_port: Arc<dyn PersonalSecretaryStoreT> = store.clone();
        let metrics = Arc::new(IngestionMetrics::default());
        let (queue, worker) = spawn_ingestion_worker(
            store_port,
            ConnectionEpochId::new("epoch-poison").unwrap(),
            config,
            None,
            None,
            0,
            None,
            Some(Arc::clone(&metrics)),
        );

        queue.try_enqueue(message("msg-ok-1")).unwrap();
        queue.try_enqueue(message("msg-poison")).unwrap();
        queue.try_enqueue(message("msg-ok-2")).unwrap();
        drop(queue);
        let report = worker.await.unwrap();

        assert_eq!(report.accepted, 2, "两个合法邻居应全部提交");
        assert_eq!(report.invalid, 1, "一条 poison 应被标记 invalid");
        assert_eq!(report.duplicates, 0);
    }

    #[tokio::test]
    async fn micro_batch_commits_multiple_messages_in_single_transaction() {
        let store = Arc::new(BatchStore {
            results: Mutex::new(VecDeque::from(vec![vec![
                accepted("evt-a"),
                accepted("evt-b"),
                duplicate("evt-c"),
            ]])),
            errors: Mutex::new(VecDeque::new()),
            insert_attempts: AtomicU64::new(0),
            max_batch_size: AtomicU64::new(0),
        });
        let config = IngestionConfig {
            queue_capacity: 8,
            batch_size: 64,
            batch_flush_ms: 5,
            retry_initial_ms: 1,
            retry_max_ms: 2,
            shutdown_drain_timeout_secs: 1,
        };
        let store_port: Arc<dyn PersonalSecretaryStoreT> = store.clone();
        let metrics = Arc::new(IngestionMetrics::default());
        let (queue, worker) = spawn_ingestion_worker(
            store_port,
            ConnectionEpochId::new("epoch-batch").unwrap(),
            config,
            None,
            None,
            0,
            None,
            Some(Arc::clone(&metrics)),
        );

        for i in 0..3 {
            queue
                .try_enqueue(message(&format!("batch-msg-{i}")))
                .unwrap();
        }
        drop(queue);
        let report = worker.await.unwrap();

        assert_eq!(report.accepted, 2);
        assert_eq!(report.duplicates, 1);
        assert_eq!(report.batches_committed, 1, "3 条消息应在 1 个批次中提交");
        assert_eq!(store.insert_attempts.load(Ordering::Acquire), 1);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.enqueued, 3);
        assert_eq!(snapshot.committed, 3);
        assert_eq!(snapshot.commit_latency_count, 3);
        assert!(
            snapshot.commit_latency_max_ms >= snapshot.last_commit_latency_ms,
            "max latency must include the latest committed message"
        );
    }

    /// 测试场景 1 的扩展：1000 条合成消息 → 多个批次。
    #[tokio::test]
    async fn synthetic_1000_messages_produce_multiple_batches_and_correct_counts() {
        let accepted_results: Vec<Vec<IngestMessageOutcome>> = (0..20)
            .map(|batch_idx| {
                (0..50)
                    .map(|i| accepted(&format!("evt-b{batch_idx}-m{i}")))
                    .collect()
            })
            .collect();
        let store = Arc::new(BatchStore {
            results: Mutex::new(VecDeque::from(accepted_results)),
            errors: Mutex::new(VecDeque::new()),
            insert_attempts: AtomicU64::new(0),
            max_batch_size: AtomicU64::new(0),
        });
        let config = IngestionConfig {
            queue_capacity: 1024,
            batch_size: 50,
            batch_flush_ms: 3,
            retry_initial_ms: 1,
            retry_max_ms: 2,
            shutdown_drain_timeout_secs: 1,
        };
        let store_port: Arc<dyn PersonalSecretaryStoreT> = store.clone();
        let (queue, worker) = spawn_ingestion_worker(
            store_port,
            ConnectionEpochId::new("epoch-1k").unwrap(),
            config,
            None,
            None,
            0,
            None,
            None,
        );

        for i in 0..1000 {
            queue.try_enqueue(message(&format!("msg-{i:04}"))).unwrap();
        }
        drop(queue);
        let report = worker.await.unwrap();

        assert_eq!(report.accepted, 1000, "所有消息应无丢失");
        assert_eq!(report.duplicates, 0);
        assert_eq!(report.invalid, 0);
        assert!(
            report.batches_committed >= 20,
            "1000/50 = 20 批次，got batches_committed={}",
            report.batches_committed
        );
    }

    /// OPS-006：高流量群的突发 callback 必须保持有界；容量之外的消息明确返回背压，
    /// 不能无界堆积或伪装成已接收。该测试刻意在首次 await 前完成全部入队，保证负载可重复。
    #[tokio::test]
    async fn synthetic_burst_20k_is_bounded_by_queue_and_batch_limits() {
        const TOTAL_MESSAGES: usize = 20_000;
        const QUEUE_CAPACITY: usize = 512;
        const BATCH_SIZE: usize = 64;

        let accepted_results: Vec<Vec<IngestMessageOutcome>> = (0..QUEUE_CAPACITY / BATCH_SIZE)
            .map(|batch_idx| {
                (0..BATCH_SIZE)
                    .map(|index| accepted(&format!("evt-load-{batch_idx}-{index}")))
                    .collect()
            })
            .collect();
        let store = Arc::new(BatchStore {
            results: Mutex::new(VecDeque::from(accepted_results)),
            errors: Mutex::new(VecDeque::new()),
            insert_attempts: AtomicU64::new(0),
            max_batch_size: AtomicU64::new(0),
        });
        let config = IngestionConfig {
            queue_capacity: QUEUE_CAPACITY,
            batch_size: BATCH_SIZE,
            batch_flush_ms: 1,
            retry_initial_ms: 1,
            retry_max_ms: 2,
            shutdown_drain_timeout_secs: 1,
        };
        let metrics = Arc::new(IngestionMetrics::default());
        let store_port: Arc<dyn PersonalSecretaryStoreT> = store.clone();
        let (queue, worker) = spawn_ingestion_worker(
            store_port,
            ConnectionEpochId::new("epoch-load-20k").unwrap(),
            config,
            None,
            None,
            0,
            None,
            Some(Arc::clone(&metrics)),
        );

        let mut enqueued = 0_u64;
        let mut backpressured = 0_u64;
        for index in 0..TOTAL_MESSAGES {
            match queue.try_enqueue(message(&format!("load-{index:05}"))) {
                Ok(()) => enqueued += 1,
                Err(IngestionEnqueueError::Full) => backpressured += 1,
                Err(IngestionEnqueueError::Closed) => panic!("worker queue closed during burst"),
            }
        }
        assert_eq!(enqueued, QUEUE_CAPACITY as u64);
        assert_eq!(enqueued + backpressured, TOTAL_MESSAGES as u64);
        drop(queue);

        let report = worker.await.unwrap();
        let snapshot = metrics.snapshot();
        assert_eq!(report.accepted, QUEUE_CAPACITY as u64);
        assert_eq!(report.dropped, backpressured);
        assert_eq!(
            report.batches_committed,
            (QUEUE_CAPACITY / BATCH_SIZE) as u64
        );
        assert_eq!(snapshot.high_watermark, QUEUE_CAPACITY as u64);
        assert_eq!(snapshot.queue_depth, 0);
        assert_eq!(snapshot.in_flight, 0);
        assert_eq!(snapshot.committed, QUEUE_CAPACITY as u64);
        assert_eq!(
            store.max_batch_size.load(Ordering::Acquire),
            BATCH_SIZE as u64,
            "单事务微批不得超过配置上限"
        );
        assert_eq!(
            store.insert_attempts.load(Ordering::Acquire),
            (QUEUE_CAPACITY / BATCH_SIZE) as u64
        );
    }

    #[tokio::test]
    async fn queue_depth_high_watermark_tracked_across_enqueue_and_drain() {
        // 100 条消息，batch_size=50，至少 2 批。每批预填正确数量的结果。
        let results: Vec<Vec<IngestMessageOutcome>> = (0..5)
            .map(|b| {
                (0..50)
                    .map(|i| accepted(&format!("evt-depth-b{b}-m{i}")))
                    .collect()
            })
            .collect();
        let store = Arc::new(BatchStore {
            results: Mutex::new(VecDeque::from(results)),
            errors: Mutex::new(VecDeque::new()),
            insert_attempts: AtomicU64::new(0),
            max_batch_size: AtomicU64::new(0),
        });
        let config = IngestionConfig {
            queue_capacity: 256,
            batch_size: 50,
            batch_flush_ms: 5,
            retry_initial_ms: 1,
            retry_max_ms: 2,
            shutdown_drain_timeout_secs: 1,
        };
        let store_port: Arc<dyn PersonalSecretaryStoreT> = store.clone();
        let (queue, worker) = spawn_ingestion_worker(
            store_port,
            ConnectionEpochId::new("epoch-depth").unwrap(),
            config,
            None,
            None,
            0,
            None,
            None,
        );
        let metrics_clone = Arc::clone(&queue.metrics);

        for i in 0..100 {
            queue.try_enqueue(message(&format!("depth-{i}"))).unwrap();
        }
        drop(queue);
        let _ = worker.await.unwrap();

        let snapshot = metrics_clone.snapshot();
        assert_eq!(snapshot.queue_capacity, 256);
        assert!(snapshot.high_watermark >= 100, "高水位应 >= 100");
        assert_eq!(snapshot.queue_depth, 0, "Worker 排空后队列深度必须归零");
        assert_eq!(snapshot.accepted, 100);
    }
}
