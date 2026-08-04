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
    IngestionGapReason, MediaKind, PersonalSecretaryStoreT, RecallCorrelationKey, RecallUseCase,
    RichContentKind, SourceEventId,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::config::IngestionConfig;

// ── 入队时间包装（仅内存使用，不改变持久化消息模型）──────────────────────

struct TimestampedEnvelope {
    envelope: InboundMessageEnvelope,
    enqueued_at: Instant,
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
                    connection_epoch_id = %self.connection_epoch_id.as_str(),
                    platform_message_id = %platform_message_id,
                    conversation_id = %conversation_id,
                    dropped_count,
                    "个人秘书持久化队列已满，消息未入队并将创建 uncertain Gap"
                );
                Err(IngestionEnqueueError::Full)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!(
                    connection_epoch_id = %self.connection_epoch_id.as_str(),
                    platform_message_id = %platform_message_id,
                    "个人秘书持久化队列已关闭"
                );
                Err(IngestionEnqueueError::Closed)
            }
        }
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
}

impl IngestionMetrics {
    fn record_enqueue(&self, queue_capacity: usize, sender: &mpsc::Sender<TimestampedEnvelope>) {
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
        self.dropped.fetch_add(1, Ordering::AcqRel);
        self.overflow_pending.store(1, Ordering::Release);
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
        }
    }
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

    loop {
        // 批次开始前持久化溢出 Gap。
        persist_overflow_gap(&store, &connection_epoch_id, &overflow, &metrics).await;

        // 等待第一条消息（channel 关闭时退出）。
        let first = match receiver.recv().await {
            Some(msg) => msg,
            None => break,
        };
        let oldest_enqueued_age = Some(first.enqueued_at);
        let mut batch: Vec<InboundMessageEnvelope> = Vec::with_capacity(batch_capacity);
        batch.push(first.envelope);

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
                        batch.push(msg.envelope);
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
        process_batch_with_retry(
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
        )
        .await;

        metrics.record_dequeue(config.queue_capacity, receiver.len());
        metrics.in_flight.store(0, Ordering::Release);
    }

    // 排空：channel 关闭后处理剩余消息。
    let mut drain: Vec<InboundMessageEnvelope> = Vec::with_capacity(batch_capacity);
    while let Ok(msg) = receiver.try_recv() {
        drain.push(msg.envelope);
        if drain.len() >= batch_capacity {
            metrics
                .in_flight
                .store(drain.len() as u64, Ordering::Release);
            process_batch_with_retry(
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
            )
            .await;
            drain.clear();
            metrics.record_dequeue(config.queue_capacity, receiver.len());
            metrics.in_flight.store(0, Ordering::Release);
        }
    }
    if !drain.is_empty() {
        metrics
            .in_flight
            .store(drain.len() as u64, Ordering::Release);
        process_batch_with_retry(
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
        )
        .await;
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
    batch: &mut Vec<InboundMessageEnvelope>,
    batch_capacity: usize,
) {
    while batch.len() < batch_capacity {
        match receiver.try_recv() {
            Ok(msg) => batch.push(msg.envelope),
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
    batch: &[InboundMessageEnvelope],
    metrics: &Arc<IngestionMetrics>,
    _oldest_enqueued_age: Option<Instant>,
    recall_use_case: Option<&Arc<RecallUseCase>>,
    artifact_use_case: Option<&Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
    health_reporter: Option<&Arc<dyn IngestionHealthReporterT>>,
) {
    // 将整批推入二分队列（有界迭代，绝不递归爆栈）。
    let mut queue: VecDeque<Vec<InboundMessageEnvelope>> = VecDeque::new();
    queue.push_back(batch.to_vec());

    let mut retry_delay = Duration::from_millis(config.retry_initial_ms);
    let max_retry_delay = Duration::from_millis(config.retry_max_ms);

    while let Some(mut current_batch) = queue.pop_front() {
        if current_batch.is_empty() {
            continue;
        }
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

                    if let Some(health) = health_reporter {
                        health.mark_worker_success(current_unix_secs());
                    }

                    // Post-hooks 必须在事务成功后执行。
                    for (i, outcome) in outcomes.iter().enumerate() {
                        fire_post_hooks(
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
                                connection_epoch_id = %connection_epoch_id.as_str(),
                                error_code = inbound_error_code(&gap_error),
                                "无法为非法队列消息持久化 uncertain Gap"
                            );
                        }
                        tracing::warn!(
                            connection_epoch_id = %connection_epoch_id.as_str(),
                            platform_message_id = %current_batch[0].source.message_id,
                            "单条非法消息已隔离并跳过，创建 InvalidEvent uncertain Gap"
                        );
                        break; // 此子批次处理完成（跳过）
                    }
                    // 长度 > 1：事务已回滚，二分继续定位。
                    let mid = current_batch.len() / 2;
                    let right = current_batch.split_off(mid);
                    // 按原顺序：先左半后右半（保持入队顺序）。
                    queue.push_front(right);
                    queue.push_front(current_batch);
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
                            connection_epoch_id = %connection_epoch_id.as_str(),
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
}

// ── Post-hooks ───────────────────────────────────────────────────────────

async fn fire_post_hooks(
    outcome: &IngestMessageOutcome,
    message: &InboundMessageEnvelope,
    recall_use_case: Option<&Arc<RecallUseCase>>,
    artifact_use_case: Option<&Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
) {
    let source_event_id = outcome.source_event_id();
    maybe_apply_pending_recall(recall_use_case, message, source_event_id.as_str()).await;
    maybe_create_artifacts(
        artifact_use_case,
        message,
        source_event_id.as_str(),
        artifact_default_ttl_secs,
    )
    .await;
}

// ── Artifact / Recall 辅助（从旧实现迁移，逻辑不变）────────────────────

async fn maybe_create_artifacts(
    artifact_use_case: Option<&Arc<ArtifactUseCase>>,
    message: &InboundMessageEnvelope,
    source_event_id: &str,
    default_ttl_secs: u64,
) {
    let Some(use_case) = artifact_use_case else {
        return;
    };
    let Ok(source_event_id) = SourceEventId::new(source_event_id) else {
        return;
    };
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
                tracing::warn!(
                    error_code = "invalid_artifact_envelope",
                    "跳过非法 Artifact 信封"
                );
                continue;
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
            tracing::warn!(
                source_event_id = source_event_id.as_str(),
                error_code = "artifact_create_failed",
                "Artifact 创建失败（消息已入库，不回滚）"
            );
        }
    }
}

async fn maybe_apply_pending_recall(
    recall_use_case: Option<&Arc<RecallUseCase>>,
    message: &InboundMessageEnvelope,
    source_event_id: &str,
) {
    let Some(use_case) = recall_use_case else {
        return;
    };
    let correlation = match RecallCorrelationKey::new(
        message.source.account_ref(),
        message.source.channel,
        message.conversation.clone(),
        message.source.message_id.clone(),
    ) {
        Ok(key) => key,
        Err(error) => {
            let _ = error;
            tracing::warn!(
                platform_message_id = %message.source.message_id,
                error_code = "invalid_recall_correlation",
                "无法构造撤回关联键，跳过 pending tombstone 关联"
            );
            return;
        }
    };
    match use_case
        .on_message_ingested(&correlation, source_event_id)
        .await
    {
        Ok(Some(record)) => {
            tracing::debug!(
                source_event_id,
                status = record.status.as_str(),
                "消息入库后已自动应用 pending 撤回 tombstone"
            );
        }
        Ok(None) => {}
        Err(error) => {
            let _ = error;
            tracing::warn!(
                source_event_id,
                platform_message_id = %message.source.message_id,
                error_code = "pending_recall_apply_failed",
                "pending 撤回关联失败（消息已入库，不回滚）"
            );
        }
    }
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
        Ok(gap_id) => {
            overflow.mark_gap_persisted();
            metrics.overflow_pending.store(
                u64::from(overflow.gap_needs_persistence()),
                Ordering::Release,
            );
            tracing::warn!(
                connection_epoch_id = %connection_epoch_id.as_str(),
                gap_id = %gap_id.as_str(),
                dropped_count,
                "队列溢出已持久化为 uncertain Gap"
            );
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
        ConnectionEndReason, ConversationKind, ConversationRef, InboundEventStoreT,
        IngestionContinuityStoreT, IngestionGapId, MessageSource, SourceAccountRef, SourceEventId,
        SourceMessageRef, VerifiedActor, VerifiedActorKind,
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

    #[tokio::test]
    async fn database_failure_retries_in_worker_and_recovers_without_callback_blocking() {
        let store = Arc::new(BatchStore {
            results: Mutex::new(VecDeque::from(vec![vec![accepted("evt-msg-retry")]])),
            errors: Mutex::new(VecDeque::from(vec![
                InboundEventStoreError::Unavailable,
                InboundEventStoreError::Unavailable,
            ])),
            insert_attempts: AtomicU64::new(0),
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
        let (queue, worker) = spawn_ingestion_worker(
            store_port,
            ConnectionEpochId::new("epoch-poison").unwrap(),
            config,
            None,
            None,
            0,
            None,
            None,
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
        let (queue, worker) = spawn_ingestion_worker(
            store_port,
            ConnectionEpochId::new("epoch-batch").unwrap(),
            config,
            None,
            None,
            0,
            None,
            None,
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
