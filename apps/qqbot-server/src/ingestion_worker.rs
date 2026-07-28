use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use personal_secretary::{
    ArtifactEnvelope, ArtifactId, ArtifactKind, ArtifactUseCase, ConnectionEpochId, ContentSegment,
    ContentTrustLevel, InboundEventStoreError, InboundMessageEnvelope, IngestMessageOutcome,
    IngestionGapReason, MediaKind, PersonalSecretaryStoreT, RecallCorrelationKey, RecallUseCase,
    RichContentKind, SourceEventId,
};
use qqbot::napcat::NapCatError;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::config::IngestionConfig;

#[derive(Debug, Default)]
struct OverflowState {
    dropped_count: AtomicU64,
    gap_persisted: AtomicBool,
}

impl OverflowState {
    fn record_drop(&self) -> u64 {
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

pub struct IngestionQueue {
    sender: mpsc::Sender<InboundMessageEnvelope>,
    overflow: Arc<OverflowState>,
    connection_epoch_id: ConnectionEpochId,
}

impl IngestionQueue {
    /// 验收测试专用：构造一个有界队列但不启动 Worker。
    /// 发送端 drop 后所有 Receiver 立即收到 None；用于入站 handler 分支测试。
    pub fn for_test() -> Self {
        let (sender, _receiver) = mpsc::channel(1);
        Self {
            sender,
            overflow: Arc::new(OverflowState::default()),
            connection_epoch_id: ConnectionEpochId::new("test-epoch").expect("valid id"),
        }
    }

    pub fn try_enqueue(&self, message: InboundMessageEnvelope) -> Result<(), NapCatError> {
        let message = message.observed_in(self.connection_epoch_id.clone());
        let platform_message_id = message.source.message_id.clone();
        let conversation_id = message.conversation.id.clone();
        match self.sender.try_send(message) {
            Ok(()) => {
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
                tracing::warn!(
                    connection_epoch_id = %self.connection_epoch_id.as_str(),
                    platform_message_id = %platform_message_id,
                    conversation_id = %conversation_id,
                    dropped_count,
                    "个人秘书持久化队列已满，消息未入队并将创建 uncertain Gap"
                );
                Err(NapCatError::Handler(
                    "personal secretary ingestion queue is full".into(),
                ))
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!(
                    connection_epoch_id = %self.connection_epoch_id.as_str(),
                    platform_message_id = %platform_message_id,
                    "个人秘书持久化队列已关闭"
                );
                Err(NapCatError::Handler(
                    "personal secretary ingestion queue is closed".into(),
                ))
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct WorkerReport {
    pub accepted: u64,
    pub duplicates: u64,
    pub invalid: u64,
    pub retries: u64,
    pub dropped: u64,
}

pub fn spawn_ingestion_worker(
    store: Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: ConnectionEpochId,
    config: IngestionConfig,
    recall_use_case: Option<Arc<RecallUseCase>>,
    artifact_use_case: Option<Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
    health_state: Option<Arc<crate::health_runtime::RuntimeHealthState>>,
) -> (IngestionQueue, JoinHandle<WorkerReport>) {
    let (sender, receiver) = mpsc::channel(config.queue_capacity);
    let overflow = Arc::new(OverflowState::default());
    let queue = IngestionQueue {
        sender,
        overflow: Arc::clone(&overflow),
        connection_epoch_id: connection_epoch_id.clone(),
    };
    let worker = tokio::spawn(run_worker(
        receiver,
        overflow,
        store,
        connection_epoch_id,
        config,
        recall_use_case,
        artifact_use_case,
        artifact_default_ttl_secs,
        health_state,
    ));
    (queue, worker)
}

#[allow(clippy::too_many_arguments)]
async fn run_worker(
    mut receiver: mpsc::Receiver<InboundMessageEnvelope>,
    overflow: Arc<OverflowState>,
    store: Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: ConnectionEpochId,
    config: IngestionConfig,
    recall_use_case: Option<Arc<RecallUseCase>>,
    artifact_use_case: Option<Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
    health_state: Option<Arc<crate::health_runtime::RuntimeHealthState>>,
) -> WorkerReport {
    tracing::debug!(
        connection_epoch_id = %connection_epoch_id.as_str(),
        queue_capacity = config.queue_capacity,
        retry_initial_ms = config.retry_initial_ms,
        retry_max_ms = config.retry_max_ms,
        "个人秘书持久化 Worker 已启动"
    );
    let mut report = WorkerReport::default();

    while let Some(message) = receiver.recv().await {
        persist_overflow_gap(&store, &connection_epoch_id, &overflow).await;
        persist_with_retry(
            &store,
            &connection_epoch_id,
            &overflow,
            &config,
            &message,
            &mut report,
            recall_use_case.as_ref(),
            artifact_use_case.as_ref(),
            artifact_default_ttl_secs,
            health_state.as_ref(),
        )
        .await;
    }
    persist_overflow_gap(&store, &connection_epoch_id, &overflow).await;
    report.dropped = overflow.dropped_count();
    tracing::debug!(
        connection_epoch_id = %connection_epoch_id.as_str(),
        accepted = report.accepted,
        duplicates = report.duplicates,
        invalid = report.invalid,
        retries = report.retries,
        dropped = report.dropped,
        "个人秘书持久化 Worker 已排空并退出"
    );
    report
}

#[allow(clippy::too_many_arguments)]
async fn persist_with_retry(
    store: &Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: &ConnectionEpochId,
    overflow: &Arc<OverflowState>,
    config: &IngestionConfig,
    message: &InboundMessageEnvelope,
    report: &mut WorkerReport,
    recall_use_case: Option<&Arc<RecallUseCase>>,
    artifact_use_case: Option<&Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
    health_state: Option<&Arc<crate::health_runtime::RuntimeHealthState>>,
) {
    let mut attempt = 0_u64;
    let mut retry_delay = Duration::from_millis(config.retry_initial_ms);
    let max_retry_delay = Duration::from_millis(config.retry_max_ms);

    loop {
        attempt += 1;
        persist_overflow_gap(store, connection_epoch_id, overflow).await;
        tracing::trace!(
            connection_epoch_id = %connection_epoch_id.as_str(),
            platform_message_id = %message.source.message_id,
            attempt,
            "开始持久化队列消息"
        );
        match store.insert_message_if_absent(message).await {
            Ok(IngestMessageOutcome::Accepted {
                source_event_id,
                reply_to_event_id,
            }) => {
                report.accepted += 1;
                if let Some(health) = health_state {
                    health.mark_worker_success(current_unix_secs());
                }
                tracing::debug!(
                    connection_epoch_id = %connection_epoch_id.as_str(),
                    source_event_id = %source_event_id.as_str(),
                    platform_message_id = %message.source.message_id,
                    conversation_id = %message.conversation.id,
                    actor_id = %message.actor.id,
                    role = ?message.role(),
                    mention_count = message.mentioned_actor_ids().count(),
                    mention_all = message.mentions_all(),
                    reply_to_event_id = reply_to_event_id.as_ref().map(|id| id.as_str()),
                    attempt,
                    "队列消息已幂等保存，允许进入后续处理"
                );
                maybe_apply_pending_recall(recall_use_case, message, source_event_id.as_str())
                    .await;
                maybe_create_artifacts(
                    artifact_use_case,
                    message,
                    source_event_id.as_str(),
                    artifact_default_ttl_secs,
                )
                .await;
                return;
            }
            Ok(IngestMessageOutcome::Duplicate { source_event_id }) => {
                report.duplicates += 1;
                if let Some(health) = health_state {
                    health.mark_worker_success(current_unix_secs());
                }
                tracing::trace!(
                    connection_epoch_id = %connection_epoch_id.as_str(),
                    source_event_id = %source_event_id.as_str(),
                    platform_message_id = %message.source.message_id,
                    "队列消息为重复投递"
                );
                maybe_apply_pending_recall(recall_use_case, message, source_event_id.as_str())
                    .await;
                maybe_create_artifacts(
                    artifact_use_case,
                    message,
                    source_event_id.as_str(),
                    artifact_default_ttl_secs,
                )
                .await;
                return;
            }
            Err(InboundEventStoreError::InvalidData(error)) => {
                report.invalid += 1;
                if let Some(health) = health_state {
                    health.mark_worker_failure();
                }
                tracing::error!(
                    connection_epoch_id = %connection_epoch_id.as_str(),
                    platform_message_id = %message.source.message_id,
                    error = %error,
                    "队列消息不满足持久化不变量，停止重试"
                );
                if let Err(gap_error) = store
                    .mark_connection_uncertain(
                        connection_epoch_id,
                        IngestionGapReason::InvalidEvent,
                    )
                    .await
                {
                    tracing::error!(
                        connection_epoch_id = %connection_epoch_id.as_str(),
                        error = %gap_error,
                        "无法为无效队列消息持久化 uncertain Gap"
                    );
                }
                return;
            }
            Err(error) => {
                report.retries += 1;
                if let Some(health) = health_state {
                    health.mark_worker_failure();
                }
                if attempt == 1 || attempt.is_power_of_two() {
                    tracing::warn!(
                        connection_epoch_id = %connection_epoch_id.as_str(),
                        platform_message_id = %message.source.message_id,
                        attempt,
                        retry_delay_ms = retry_delay.as_millis(),
                        error = %error,
                        "消息持久化失败，将在独立 Worker 中重试"
                    );
                } else {
                    tracing::debug!(
                        connection_epoch_id = %connection_epoch_id.as_str(),
                        platform_message_id = %message.source.message_id,
                        attempt,
                        retry_delay_ms = retry_delay.as_millis(),
                        error = %error,
                        "消息持久化重试仍未成功"
                    );
                }
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay.saturating_mul(2).min(max_retry_delay);
            }
        }
    }
}

/// 富消息段 -> 有界 Artifact 信封。不下载、不写 URL 到日志。
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
                tracing::warn!(error = %error, "跳过非法 Artifact 信封");
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
            tracing::warn!(
                source_event_id = source_event_id.as_str(),
                error = %error,
                "Artifact 创建失败（消息已入库，不回滚）"
            );
        }
    }
}

/// 消息入库后尝试消费 pending tombstone。失败只记日志，不回滚已成功的消息入库。
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
            tracing::warn!(
                platform_message_id = %message.source.message_id,
                error = %error,
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
            tracing::warn!(
                source_event_id,
                platform_message_id = %message.source.message_id,
                error = %error,
                "pending 撤回关联失败（消息已入库，不回滚）"
            );
        }
    }
}

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
            tracing::warn!(
                connection_epoch_id = %connection_epoch_id.as_str(),
                gap_id = %gap_id.as_str(),
                dropped_count,
                "队列溢出已持久化为 uncertain Gap"
            );
        }
        Err(error) => tracing::debug!(
            connection_epoch_id = %connection_epoch_id.as_str(),
            dropped_count,
            error = %error,
            "队列溢出 Gap 暂未持久化，将随 Worker 重试"
        ),
    }
}

#[cfg(test)]
mod tests {
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
        let queue = IngestionQueue {
            sender,
            overflow: Arc::clone(&overflow),
            connection_epoch_id: ConnectionEpochId::new("epoch-1").unwrap(),
        };

        queue.try_enqueue(message("message-1")).unwrap();
        let error = queue.try_enqueue(message("message-2")).unwrap_err();

        assert!(matches!(error, NapCatError::Handler(_)));
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

    struct RetryStore {
        remaining_failures: AtomicU64,
        insert_attempts: AtomicU64,
    }

    #[async_trait]
    impl InboundEventStoreT for RetryStore {
        async fn insert_message_if_absent(
            &self,
            message: &InboundMessageEnvelope,
        ) -> Result<IngestMessageOutcome, InboundEventStoreError> {
            self.insert_attempts.fetch_add(1, Ordering::AcqRel);
            if self.remaining_failures.load(Ordering::Acquire) > 0 {
                self.remaining_failures.fetch_sub(1, Ordering::AcqRel);
                return Err(InboundEventStoreError::Unavailable);
            }
            Ok(IngestMessageOutcome::Accepted {
                source_event_id: SourceEventId::new(format!("event-{}", message.source.message_id))
                    .unwrap(),
                reply_to_event_id: None,
            })
        }
    }

    #[async_trait]
    impl IngestionContinuityStoreT for RetryStore {
        async fn begin_connection(
            &self,
            _account: &SourceAccountRef,
        ) -> Result<ConnectionEpochId, InboundEventStoreError> {
            ConnectionEpochId::new("epoch-retry")
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
            IngestionGapId::new("gap-retry")
                .map_err(|error| InboundEventStoreError::InvalidData(error.to_string()))
        }
    }

    #[tokio::test]
    async fn database_failure_retries_in_worker_and_recovers_without_callback_blocking() {
        let store = Arc::new(RetryStore {
            remaining_failures: AtomicU64::new(2),
            insert_attempts: AtomicU64::new(0),
        });
        let config = IngestionConfig {
            queue_capacity: 1,
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
        );

        queue.try_enqueue(message("message-retry")).unwrap();
        drop(queue);
        let report = worker.await.unwrap();

        assert_eq!(report.accepted, 1);
        assert_eq!(report.retries, 2);
        assert_eq!(store.insert_attempts.load(Ordering::Acquire), 3);
    }
}
