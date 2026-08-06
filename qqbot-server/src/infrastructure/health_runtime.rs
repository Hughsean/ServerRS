//! B7 runtime health sampling, cached snapshots and structured logging.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use personal_secretary::{
    HealthAggregator, HealthSnapshot, HealthSnapshotProducer, HealthStatus, SourceAccountRef,
    SubsystemHealth,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::config::HealthConfig;
use crate::recall::RecallSpoolTelemetry;
use crate::worker_lifecycle::WorkerHandle;

const UNKNOWN: u8 = 0;
const HEALTHY: u8 = 1;
const DEGRADED: u8 = 2;

const FAILURE_NONE: u8 = 0;
const FAILURE_SAMPLE_FAILED: u8 = 1;
const FAILURE_QUEUE_OVERFLOW: u8 = 2;
const FAILURE_DATABASE_UNAVAILABLE: u8 = 3;
const FAILURE_HISTORY_UNPROVABLE: u8 = 4;
const FAILURE_INVALID_EVENT: u8 = 5;
const FAILURE_UNKNOWN: u8 = 6;

#[derive(Debug, Clone, Copy, Default)]
struct BackfillHealthSample {
    uncertain_gaps: u64,
    backfilling_gaps: u64,
    unrecoverable_gaps: u64,
    pages_read: u64,
    events_read: u64,
    accepted: u64,
    duplicates: u64,
    anomalies: u64,
    budget_exhausted_runs: u64,
    failure_code: u8,
    thread_misassociation_feedback: u64,
    reminder_false_positive_feedback: u64,
}

#[derive(Debug, Default)]
pub struct RuntimeHealthState {
    websocket_observed: AtomicBool,
    websocket_connected: AtomicBool,
    history_observed: AtomicBool,
    uncertain_gaps: AtomicU64,
    backfilling_gaps: AtomicU64,
    unrecoverable_gaps: AtomicU64,
    backfill_pages_read: AtomicU64,
    backfill_events_read: AtomicU64,
    backfill_accepted: AtomicU64,
    backfill_duplicates: AtomicU64,
    backfill_anomalies: AtomicU64,
    backfill_budget_exhausted_runs: AtomicU64,
    backfill_failure_code: AtomicU8,
    database_state: AtomicU8,
    workers_state: AtomicU8,
    worker_started: AtomicBool,
    last_database_success_unix: AtomicU64,
    last_worker_success_unix: AtomicU64,
    thread_misassociation_feedback: AtomicU64,
    reminder_false_positive_feedback: AtomicU64,
}

impl RuntimeHealthState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn set_websocket_connected(&self, connected: bool) {
        self.websocket_observed.store(true, Ordering::Release);
        self.websocket_connected.store(connected, Ordering::Release);
    }

    pub fn set_uncertain_gaps(&self, count: u64) {
        self.history_observed.store(true, Ordering::Release);
        self.uncertain_gaps.store(count, Ordering::Release);
    }

    pub fn mark_worker_started(&self) {
        self.worker_started.store(true, Ordering::Release);
        self.workers_state.store(HEALTHY, Ordering::Release);
    }

    pub fn mark_worker_success(&self, now_unix: u64) {
        self.worker_started.store(true, Ordering::Release);
        self.workers_state.store(HEALTHY, Ordering::Release);
        self.last_worker_success_unix
            .store(now_unix, Ordering::Release);
    }

    pub fn mark_worker_failure(&self) {
        self.worker_started.store(true, Ordering::Release);
        self.workers_state.store(DEGRADED, Ordering::Release);
    }

    fn mark_database_sample(&self, now_unix: u64, sample: BackfillHealthSample) {
        self.database_state.store(HEALTHY, Ordering::Release);
        self.last_database_success_unix
            .store(now_unix, Ordering::Release);
        self.history_observed.store(true, Ordering::Release);
        self.uncertain_gaps
            .store(sample.uncertain_gaps, Ordering::Release);
        self.backfilling_gaps
            .store(sample.backfilling_gaps, Ordering::Release);
        self.unrecoverable_gaps
            .store(sample.unrecoverable_gaps, Ordering::Release);
        self.backfill_pages_read
            .store(sample.pages_read, Ordering::Release);
        self.backfill_events_read
            .store(sample.events_read, Ordering::Release);
        self.backfill_accepted
            .store(sample.accepted, Ordering::Release);
        self.backfill_duplicates
            .store(sample.duplicates, Ordering::Release);
        self.backfill_anomalies
            .store(sample.anomalies, Ordering::Release);
        self.backfill_budget_exhausted_runs
            .store(sample.budget_exhausted_runs, Ordering::Release);
        self.backfill_failure_code
            .store(sample.failure_code, Ordering::Release);
        self.thread_misassociation_feedback
            .store(sample.thread_misassociation_feedback, Ordering::Release);
        self.reminder_false_positive_feedback
            .store(sample.reminder_false_positive_feedback, Ordering::Release);
    }

    fn mark_database_failure(&self) {
        self.database_state.store(DEGRADED, Ordering::Release);
        self.history_observed.store(false, Ordering::Release);
        self.backfill_failure_code
            .store(FAILURE_SAMPLE_FAILED, Ordering::Release);
    }
}

impl crate::ingestion_worker::IngestionHealthReporterT for RuntimeHealthState {
    fn mark_worker_success(&self, now_unix: u64) {
        Self::mark_worker_success(self, now_unix);
    }

    fn mark_worker_failure(&self) {
        Self::mark_worker_failure(self);
    }
}

struct WebsocketProducer(Arc<RuntimeHealthState>);
struct HistoryProducer(Arc<RuntimeHealthState>);
struct DatabaseProducer(Arc<RuntimeHealthState>);
struct RecallSpoolProducer(Arc<RecallSpoolTelemetry>);
struct RealtimeSpoolProducer(Arc<crate::realtime_spool::RealtimeSpoolTelemetry>);
struct IngestionMetricsProducer(Arc<crate::ingestion_worker::IngestionMetrics>);
struct LlmMetricsProducer {
    metrics: Arc<crate::llm::LlmMetrics>,
    input_price_microusd_per_million_tokens: Option<u64>,
    output_price_microusd_per_million_tokens: Option<u64>,
}
struct FeedbackProducer(Arc<RuntimeHealthState>);
struct WorkerProducer {
    state: Arc<RuntimeHealthState>,
    stale_secs: u64,
}

#[async_trait]
impl HealthSnapshotProducer for WebsocketProducer {
    fn name(&self) -> &'static str {
        "websocket"
    }

    async fn health(&self) -> SubsystemHealth {
        let observed = self.0.websocket_observed.load(Ordering::Acquire);
        let connected = self.0.websocket_connected.load(Ordering::Acquire);
        SubsystemHealth {
            name: self.name().into(),
            status: if !observed {
                HealthStatus::Uncertain
            } else if connected {
                HealthStatus::Healthy
            } else {
                HealthStatus::Degraded
            },
            last_success_at_unix_secs: None,
            last_error: (!observed)
                .then(|| "not_observed".into())
                .or_else(|| (!connected).then(|| "websocket_disconnected".into())),
            metrics: BTreeMap::new(),
        }
    }
}

#[async_trait]
impl HealthSnapshotProducer for HistoryProducer {
    fn name(&self) -> &'static str {
        "history_completeness"
    }

    async fn health(&self) -> SubsystemHealth {
        let observed = self.0.history_observed.load(Ordering::Acquire);
        let uncertain_gaps = self.0.uncertain_gaps.load(Ordering::Acquire);
        let backfilling_gaps = self.0.backfilling_gaps.load(Ordering::Acquire);
        let unrecoverable_gaps = self.0.unrecoverable_gaps.load(Ordering::Acquire);
        let mut metrics = BTreeMap::new();
        metrics.insert("uncertain_gaps".into(), uncertain_gaps);
        metrics.insert("backfilling_gaps".into(), backfilling_gaps);
        metrics.insert("unrecoverable_gaps".into(), unrecoverable_gaps);
        metrics.insert(
            "backfill_pages_read".into(),
            self.0.backfill_pages_read.load(Ordering::Acquire),
        );
        metrics.insert(
            "backfill_events_read".into(),
            self.0.backfill_events_read.load(Ordering::Acquire),
        );
        metrics.insert(
            "backfill_accepted".into(),
            self.0.backfill_accepted.load(Ordering::Acquire),
        );
        metrics.insert(
            "backfill_duplicates".into(),
            self.0.backfill_duplicates.load(Ordering::Acquire),
        );
        metrics.insert(
            "backfill_anomalies".into(),
            self.0.backfill_anomalies.load(Ordering::Acquire),
        );
        metrics.insert(
            "backfill_budget_exhausted_runs".into(),
            self.0
                .backfill_budget_exhausted_runs
                .load(Ordering::Acquire),
        );
        let failure_code = self.0.backfill_failure_code.load(Ordering::Acquire);
        SubsystemHealth {
            name: self.name().into(),
            status: if !observed {
                HealthStatus::Uncertain
            } else if uncertain_gaps == 0 && backfilling_gaps == 0 && unrecoverable_gaps == 0 {
                HealthStatus::Healthy
            } else {
                HealthStatus::Degraded
            },
            last_success_at_unix_secs: nonzero_time(
                self.0.last_database_success_unix.load(Ordering::Acquire),
            ),
            last_error: if !observed {
                Some(
                    if failure_code == FAILURE_SAMPLE_FAILED {
                        "backfill_sample_failed"
                    } else {
                        "not_observed"
                    }
                    .into(),
                )
            } else if uncertain_gaps == 0 && backfilling_gaps == 0 && unrecoverable_gaps == 0 {
                None
            } else {
                history_failure_code(failure_code)
                    .map(str::to_owned)
                    .or_else(|| {
                        (unrecoverable_gaps > 0).then_some("unrecoverable_gaps_present".into())
                    })
                    .or_else(|| (uncertain_gaps > 0).then_some("uncertain_gaps_present".into()))
            },
            metrics,
        }
    }
}

fn history_failure_code(code: u8) -> Option<&'static str> {
    match code {
        FAILURE_QUEUE_OVERFLOW => Some("backfill_queue_overflow"),
        FAILURE_DATABASE_UNAVAILABLE => Some("backfill_database_unavailable"),
        FAILURE_HISTORY_UNPROVABLE => Some("backfill_history_unprovable"),
        FAILURE_INVALID_EVENT => Some("backfill_invalid_event"),
        FAILURE_UNKNOWN => Some("backfill_failure_unknown"),
        _ => None,
    }
}

#[async_trait]
impl HealthSnapshotProducer for DatabaseProducer {
    fn name(&self) -> &'static str {
        "mysql"
    }

    async fn health(&self) -> SubsystemHealth {
        let state = self.0.database_state.load(Ordering::Acquire);
        SubsystemHealth {
            name: self.name().into(),
            status: state_status(state),
            last_success_at_unix_secs: nonzero_time(
                self.0.last_database_success_unix.load(Ordering::Acquire),
            ),
            last_error: match state {
                UNKNOWN => Some("not_observed".into()),
                DEGRADED => Some("database_unavailable".into()),
                _ => None,
            },
            metrics: BTreeMap::new(),
        }
    }
}

#[async_trait]
impl HealthSnapshotProducer for FeedbackProducer {
    fn name(&self) -> &'static str {
        "owner_feedback"
    }

    async fn health(&self) -> SubsystemHealth {
        let observed = self.0.history_observed.load(Ordering::Acquire);
        let mut metrics = BTreeMap::new();
        metrics.insert(
            "thread_misassociation_feedback".into(),
            self.0
                .thread_misassociation_feedback
                .load(Ordering::Acquire),
        );
        metrics.insert(
            "reminder_false_positive_feedback".into(),
            self.0
                .reminder_false_positive_feedback
                .load(Ordering::Acquire),
        );
        SubsystemHealth {
            name: self.name().into(),
            status: if observed {
                HealthStatus::Healthy
            } else {
                HealthStatus::Uncertain
            },
            last_success_at_unix_secs: nonzero_time(
                self.0.last_database_success_unix.load(Ordering::Acquire),
            ),
            last_error: (!observed).then(|| "not_observed".into()),
            metrics,
        }
    }
}

#[async_trait]
impl HealthSnapshotProducer for WorkerProducer {
    fn name(&self) -> &'static str {
        "workers"
    }

    async fn health(&self) -> SubsystemHealth {
        let state = self.state.workers_state.load(Ordering::Acquire);
        let started = self.state.worker_started.load(Ordering::Acquire);
        let last_success = self.state.last_worker_success_unix.load(Ordering::Acquire);
        let now = now_unix().max(0) as u64;
        let stale = last_success > 0 && now.saturating_sub(last_success) > self.stale_secs;
        SubsystemHealth {
            name: self.name().into(),
            status: if stale {
                HealthStatus::Degraded
            } else if !started {
                HealthStatus::Uncertain
            } else {
                state_status(state)
            },
            last_success_at_unix_secs: nonzero_time(last_success),
            last_error: if stale {
                Some("worker_success_stale".into())
            } else {
                match state {
                    _ if !started => Some("worker_not_started".into()),
                    DEGRADED => Some("worker_failure".into()),
                    _ if last_success == 0 => Some("worker_started_idle".into()),
                    _ => None,
                }
            },
            metrics: BTreeMap::new(),
        }
    }
}

#[async_trait]
impl HealthSnapshotProducer for RecallSpoolProducer {
    fn name(&self) -> &'static str {
        "recall_spool"
    }

    async fn health(&self) -> SubsystemHealth {
        let snapshot = self.0.snapshot();
        let now = now_unix().max(0) as u64;
        let mut metrics = BTreeMap::new();
        metrics.insert("backlog".into(), snapshot.pending_frames);
        metrics.insert("capacity_bytes".into(), snapshot.capacity_bytes);
        metrics.insert("bytes_used".into(), snapshot.bytes_used);
        metrics.insert("quarantine_count".into(), snapshot.quarantine_count);
        let ratio = snapshot
            .bytes_used
            .saturating_mul(10_000)
            .checked_div(snapshot.capacity_bytes)
            .unwrap_or(0)
            .min(10_000);
        metrics.insert("capacity_used_ratio_bps".into(), ratio);
        if let Some(oldest) = snapshot.oldest_occurred_at_unix_secs {
            metrics.insert(
                "oldest_record_age_secs".into(),
                now.saturating_sub(oldest.max(0) as u64),
            );
        }
        SubsystemHealth {
            name: self.name().into(),
            status: if !snapshot.observed {
                HealthStatus::Uncertain
            } else if !snapshot.usable {
                HealthStatus::Unavailable
            } else if snapshot.degraded {
                HealthStatus::Degraded
            } else {
                HealthStatus::Healthy
            },
            last_success_at_unix_secs: snapshot
                .last_drain_success_unix_secs
                .or(snapshot.last_append_success_unix_secs),
            last_error: snapshot.recent_error_code.map(str::to_owned),
            metrics,
        }
    }
}

#[async_trait]
impl HealthSnapshotProducer for RealtimeSpoolProducer {
    fn name(&self) -> &'static str {
        "realtime_spool"
    }

    async fn health(&self) -> SubsystemHealth {
        let snapshot = self.0.snapshot();
        let now = now_unix().max(0) as u64;
        let mut metrics = BTreeMap::new();
        metrics.insert("pending_frames".into(), snapshot.pending_frames);
        metrics.insert("capacity_bytes".into(), snapshot.capacity_bytes);
        metrics.insert("bytes_used".into(), snapshot.bytes_used);
        metrics.insert("quarantine_count".into(), snapshot.quarantine_count);
        metrics.insert(
            "reconciliation_pending".into(),
            u64::from(snapshot.reconciliation_pending),
        );
        let ratio = snapshot
            .bytes_used
            .saturating_mul(10_000)
            .checked_div(snapshot.capacity_bytes)
            .unwrap_or(0)
            .min(10_000);
        metrics.insert("capacity_used_ratio_bps".into(), ratio);
        if let Some(oldest) = snapshot.oldest_occurred_at_unix_secs {
            metrics.insert(
                "oldest_record_age_secs".into(),
                now.saturating_sub(oldest.max(0) as u64),
            );
        }
        SubsystemHealth {
            name: self.name().into(),
            status: if !snapshot.observed {
                HealthStatus::Uncertain
            } else if !snapshot.usable {
                HealthStatus::Unavailable
            } else if snapshot.reconciliation_pending || snapshot.pending_frames > 0 {
                HealthStatus::Degraded
            } else {
                HealthStatus::Healthy
            },
            last_success_at_unix_secs: None,
            last_error: snapshot.recent_error_code.map(str::to_owned),
            metrics,
        }
    }
}
#[async_trait]
impl HealthSnapshotProducer for IngestionMetricsProducer {
    fn name(&self) -> &'static str {
        "ingestion"
    }

    async fn health(&self) -> SubsystemHealth {
        let snapshot = self.0.snapshot();
        let mut metrics = BTreeMap::new();
        metrics.insert("queue_capacity".into(), snapshot.queue_capacity);
        metrics.insert("queue_depth".into(), snapshot.queue_depth);
        metrics.insert("in_flight".into(), snapshot.in_flight);
        metrics.insert("high_watermark".into(), snapshot.high_watermark);
        metrics.insert("accepted".into(), snapshot.accepted);
        metrics.insert("duplicates".into(), snapshot.duplicates);
        metrics.insert("invalid".into(), snapshot.invalid);
        metrics.insert("dropped".into(), snapshot.dropped);
        metrics.insert("batches_committed".into(), snapshot.batches_committed);
        metrics.insert("last_batch_size".into(), snapshot.last_batch_size);
        metrics.insert("overflow_pending".into(), snapshot.overflow_pending);
        metrics.insert("enqueued".into(), snapshot.enqueued);
        metrics.insert("committed".into(), snapshot.committed);
        metrics.insert("commit_latency_count".into(), snapshot.commit_latency_count);
        metrics.insert(
            "commit_latency_sum_ms".into(),
            snapshot.commit_latency_sum_ms,
        );
        metrics.insert(
            "commit_latency_max_ms".into(),
            snapshot.commit_latency_max_ms,
        );
        metrics.insert(
            "last_commit_latency_ms".into(),
            snapshot.last_commit_latency_ms,
        );
        metrics.insert(
            "commit_latency_avg_ms_x1000".into(),
            snapshot
                .commit_latency_sum_ms
                .saturating_mul(1_000)
                .checked_div(snapshot.commit_latency_count.max(1))
                .unwrap_or(0),
        );
        let status = if snapshot.last_failure_at > snapshot.last_success_at {
            HealthStatus::Degraded
        } else if snapshot.accepted == 0 && snapshot.duplicates == 0 && snapshot.invalid == 0 {
            HealthStatus::Uncertain
        } else {
            HealthStatus::Healthy
        };
        SubsystemHealth {
            name: self.name().into(),
            status,
            last_success_at_unix_secs: nonzero_time(snapshot.last_success_at),
            last_error: (snapshot.overflow_pending > 0).then(|| "overflow_detected".into()),
            metrics,
        }
    }
}

fn token_cost_microusd(tokens: u64, price: Option<u64>) -> u64 {
    price
        .map(|price| {
            ((tokens as u128)
                .saturating_mul(price as u128)
                .checked_div(1_000_000)
                .unwrap_or(0))
            .min(u64::MAX as u128) as u64
        })
        .unwrap_or(0)
}

#[async_trait]
impl HealthSnapshotProducer for LlmMetricsProducer {
    fn name(&self) -> &'static str {
        "llm"
    }

    async fn health(&self) -> SubsystemHealth {
        let snapshot = self.metrics.snapshot();
        let mut metrics = BTreeMap::new();
        metrics.insert("calls_total".into(), snapshot.calls);
        metrics.insert("successes".into(), snapshot.successes);
        metrics.insert("failures".into(), snapshot.failures);
        metrics.insert("usage_missing".into(), snapshot.usage_missing);
        metrics.insert("prompt_tokens".into(), snapshot.prompt_tokens);
        metrics.insert("completion_tokens".into(), snapshot.completion_tokens);
        metrics.insert("total_tokens".into(), snapshot.total_tokens);
        metrics.insert("latency_count".into(), snapshot.latency_count);
        metrics.insert("latency_sum_ms".into(), snapshot.latency_sum_ms);
        metrics.insert("latency_max_ms".into(), snapshot.latency_max_ms);
        metrics.insert(
            "latency_avg_ms_x1000".into(),
            snapshot
                .latency_sum_ms
                .saturating_mul(1_000)
                .checked_div(snapshot.latency_count.max(1))
                .unwrap_or(0),
        );
        let cost_configured = u64::from(
            self.input_price_microusd_per_million_tokens.is_some()
                && self.output_price_microusd_per_million_tokens.is_some(),
        );
        metrics.insert("cost_configured".into(), cost_configured);
        if cost_configured == 1 {
            metrics.insert(
                "estimated_cost_microusd".into(),
                token_cost_microusd(
                    snapshot.prompt_tokens,
                    self.input_price_microusd_per_million_tokens,
                )
                .saturating_add(token_cost_microusd(
                    snapshot.completion_tokens,
                    self.output_price_microusd_per_million_tokens,
                )),
            );
        }
        SubsystemHealth {
            name: self.name().into(),
            status: if snapshot.calls == 0 {
                HealthStatus::Uncertain
            } else if snapshot.failures > 0 && snapshot.successes == 0 {
                HealthStatus::Degraded
            } else {
                HealthStatus::Healthy
            },
            last_success_at_unix_secs: None,
            last_error: (snapshot.failures > 0 && snapshot.successes == 0)
                .then(|| "llm_calls_failed".into()),
            metrics,
        }
    }
}

fn state_status(state: u8) -> HealthStatus {
    match state {
        HEALTHY => HealthStatus::Healthy,
        DEGRADED => HealthStatus::Degraded,
        _ => HealthStatus::Uncertain,
    }
}

fn nonzero_time(value: u64) -> Option<i64> {
    (value > 0).then_some(value.min(i64::MAX as u64) as i64)
}

pub fn build_runtime_health_aggregator(
    state: Arc<RuntimeHealthState>,
    cache_ttl_secs: u64,
    worker_success_stale_secs: u64,
    ingestion_metrics: Option<Arc<crate::ingestion_worker::IngestionMetrics>>,
) -> HealthAggregator {
    let mut aggregator = HealthAggregator::new(Duration::from_secs(cache_ttl_secs.max(1)));
    aggregator.add_producer(Arc::new(WebsocketProducer(Arc::clone(&state))));
    aggregator.add_producer(Arc::new(HistoryProducer(Arc::clone(&state))));
    aggregator.add_producer(Arc::new(FeedbackProducer(Arc::clone(&state))));
    aggregator.add_producer(Arc::new(DatabaseProducer(Arc::clone(&state))));
    aggregator.add_producer(Arc::new(WorkerProducer {
        state,
        stale_secs: worker_success_stale_secs.max(1),
    }));
    if let Some(metrics) = ingestion_metrics {
        aggregator.add_producer(Arc::new(IngestionMetricsProducer(metrics)));
    }
    aggregator
}

pub fn build_runtime_health_aggregator_with_recall_spool(
    state: Arc<RuntimeHealthState>,
    recall_spool: Arc<RecallSpoolTelemetry>,
    cache_ttl_secs: u64,
    worker_success_stale_secs: u64,
    ingestion_metrics: Option<Arc<crate::ingestion_worker::IngestionMetrics>>,
) -> HealthAggregator {
    let mut aggregator = HealthAggregator::new(Duration::from_secs(cache_ttl_secs.max(1)));
    aggregator.add_producer(Arc::new(WebsocketProducer(Arc::clone(&state))));
    aggregator.add_producer(Arc::new(HistoryProducer(Arc::clone(&state))));
    aggregator.add_producer(Arc::new(FeedbackProducer(Arc::clone(&state))));
    aggregator.add_producer(Arc::new(DatabaseProducer(Arc::clone(&state))));
    aggregator.add_producer(Arc::new(RecallSpoolProducer(recall_spool)));
    aggregator.add_producer(Arc::new(WorkerProducer {
        state,
        stale_secs: worker_success_stale_secs.max(1),
    }));
    if let Some(metrics) = ingestion_metrics {
        aggregator.add_producer(Arc::new(IngestionMetricsProducer(metrics)));
    }
    aggregator
}

pub fn build_runtime_health_aggregator_with_spools(
    state: Arc<RuntimeHealthState>,
    recall_spool: Arc<RecallSpoolTelemetry>,
    realtime_spool: Option<Arc<crate::realtime_spool::RealtimeSpoolTelemetry>>,
    cache_ttl_secs: u64,
    worker_success_stale_secs: u64,
    ingestion_metrics: Option<Arc<crate::ingestion_worker::IngestionMetrics>>,
) -> HealthAggregator {
    build_runtime_health_aggregator_with_spools_and_llm(
        state,
        recall_spool,
        realtime_spool,
        cache_ttl_secs,
        worker_success_stale_secs,
        ingestion_metrics,
        None,
    )
}

pub(crate) struct LlmHealthMetricsConfig {
    pub metrics: Arc<crate::llm::LlmMetrics>,
    pub input_price_microusd_per_million_tokens: Option<u64>,
    pub output_price_microusd_per_million_tokens: Option<u64>,
}

pub(crate) fn build_runtime_health_aggregator_with_spools_and_llm(
    state: Arc<RuntimeHealthState>,
    recall_spool: Arc<RecallSpoolTelemetry>,
    realtime_spool: Option<Arc<crate::realtime_spool::RealtimeSpoolTelemetry>>,
    cache_ttl_secs: u64,
    worker_success_stale_secs: u64,
    ingestion_metrics: Option<Arc<crate::ingestion_worker::IngestionMetrics>>,
    llm: Option<LlmHealthMetricsConfig>,
) -> HealthAggregator {
    let mut aggregator = build_runtime_health_aggregator_with_recall_spool(
        Arc::clone(&state),
        recall_spool,
        cache_ttl_secs,
        worker_success_stale_secs,
        ingestion_metrics,
    );
    if let Some(telemetry) = realtime_spool {
        aggregator.add_producer(Arc::new(RealtimeSpoolProducer(telemetry)));
    }
    if let Some(llm) = llm {
        aggregator.add_producer(Arc::new(LlmMetricsProducer {
            metrics: llm.metrics,
            input_price_microusd_per_million_tokens: llm.input_price_microusd_per_million_tokens,
            output_price_microusd_per_million_tokens: llm.output_price_microusd_per_million_tokens,
        }));
    }
    aggregator
}

#[cfg(test)]
mod tests {
    use super::*;

    #[path = "../../../../personal-secretary-mysql/tests/common/mod.rs"]
    mod mysql_common;

    #[tokio::test]
    async fn recall_spool_producer_reports_bounded_numeric_metrics() {
        let telemetry = RecallSpoolTelemetry::new(1_000);
        telemetry.set_test_snapshot(250, 3, 2);
        let health = RecallSpoolProducer(telemetry).health().await;
        assert_eq!(health.status, HealthStatus::Healthy);
        assert_eq!(health.metrics.get("backlog"), Some(&3));
        assert_eq!(health.metrics.get("capacity_used_ratio_bps"), Some(&2_500));
        assert_eq!(health.metrics.get("quarantine_count"), Some(&2));
        assert_eq!(health.last_error, None);
    }

    #[tokio::test]
    async fn ingestion_health_recovers_after_a_later_success() {
        let metrics = Arc::new(crate::ingestion_worker::IngestionMetrics::default());
        metrics.accepted.store(1, Ordering::Release);
        metrics.last_failure_at.store(20, Ordering::Release);
        let producer = IngestionMetricsProducer(Arc::clone(&metrics));
        assert_eq!(producer.health().await.status, HealthStatus::Degraded);

        metrics.last_success_at.store(21, Ordering::Release);
        let health = producer.health().await;
        assert_eq!(health.status, HealthStatus::Healthy);
        assert_eq!(health.last_error, None);
    }

    #[tokio::test]
    async fn ingestion_health_exposes_throughput_and_latency_without_identity_labels() {
        let metrics = Arc::new(crate::ingestion_worker::IngestionMetrics::default());
        metrics.enqueued.store(20, Ordering::Release);
        metrics.committed.store(18, Ordering::Release);
        metrics.commit_latency_count.store(18, Ordering::Release);
        metrics.commit_latency_sum_ms.store(900, Ordering::Release);
        metrics.commit_latency_max_ms.store(120, Ordering::Release);
        metrics.accepted.store(18, Ordering::Release);
        let health = IngestionMetricsProducer(metrics).health().await;
        assert_eq!(health.metrics.get("enqueued"), Some(&20));
        assert_eq!(health.metrics.get("committed"), Some(&18));
        assert_eq!(health.metrics.get("commit_latency_max_ms"), Some(&120));
        assert!(health.metrics.keys().all(|key| !key.contains("account")));
    }

    #[tokio::test]
    async fn llm_health_reports_configured_cost_only_with_prices() {
        let metrics = Arc::new(crate::llm::LlmMetrics::default());
        let response = Ok(crate::llm::StructuredLlmResponse {
            value: serde_json::json!({}),
            usage: crate::llm::LlmUsage {
                prompt_tokens: Some(1_000_000),
                completion_tokens: Some(500_000),
                total_tokens: Some(1_500_000),
            },
        });
        metrics.record_for_test(&response);
        let producer = LlmMetricsProducer {
            metrics,
            input_price_microusd_per_million_tokens: Some(2_000_000),
            output_price_microusd_per_million_tokens: Some(4_000_000),
        };
        let health = producer.health().await;
        assert_eq!(
            health.metrics.get("estimated_cost_microusd"),
            Some(&4_000_000)
        );
        assert_eq!(health.metrics.get("cost_configured"), Some(&1));
    }

    #[tokio::test]
    async fn history_health_reports_bounded_backfill_progress_and_failure_code() {
        let state = RuntimeHealthState::new();
        state.mark_database_sample(
            123,
            BackfillHealthSample {
                uncertain_gaps: 2,
                backfilling_gaps: 1,
                unrecoverable_gaps: 0,
                pages_read: 7,
                events_read: 80,
                accepted: 12,
                duplicates: 68,
                anomalies: 1,
                budget_exhausted_runs: 1,
                failure_code: FAILURE_HISTORY_UNPROVABLE,
                thread_misassociation_feedback: 0,
                reminder_false_positive_feedback: 0,
            },
        );

        let health = HistoryProducer(state).health().await;
        assert_eq!(health.status, HealthStatus::Degraded);
        assert_eq!(health.last_success_at_unix_secs, Some(123));
        assert_eq!(
            health.last_error.as_deref(),
            Some("backfill_history_unprovable")
        );
        assert_eq!(health.metrics.get("uncertain_gaps"), Some(&2));
        assert_eq!(health.metrics.get("backfilling_gaps"), Some(&1));
        assert_eq!(health.metrics.get("backfill_pages_read"), Some(&7));
        assert_eq!(health.metrics.get("backfill_events_read"), Some(&80));
        assert_eq!(health.metrics.get("backfill_accepted"), Some(&12));
        assert_eq!(health.metrics.get("backfill_duplicates"), Some(&68));
        assert_eq!(health.metrics.get("backfill_anomalies"), Some(&1));
        assert_eq!(
            health.metrics.get("backfill_budget_exhausted_runs"),
            Some(&1)
        );
    }

    #[tokio::test]
    async fn history_health_redacts_unknown_failure_text_and_sampling_errors() {
        let state = RuntimeHealthState::new();
        state.mark_database_sample(
            123,
            BackfillHealthSample {
                uncertain_gaps: 1,
                failure_code: failure_code_from_text("mysql://secret/path"),
                ..BackfillHealthSample::default()
            },
        );
        let health = HistoryProducer(Arc::clone(&state)).health().await;
        assert_eq!(
            health.last_error.as_deref(),
            Some("backfill_failure_unknown")
        );

        state.mark_database_sample(
            124,
            BackfillHealthSample {
                failure_code: FAILURE_HISTORY_UNPROVABLE,
                ..BackfillHealthSample::default()
            },
        );
        let health = HistoryProducer(Arc::clone(&state)).health().await;
        assert_eq!(health.status, HealthStatus::Healthy);
        assert_eq!(health.last_error, None);

        state.mark_database_failure();
        let health = HistoryProducer(state).health().await;
        assert_eq!(health.status, HealthStatus::Uncertain);
        assert_eq!(health.last_error.as_deref(), Some("backfill_sample_failed"));
    }

    #[tokio::test]
    #[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
    async fn production_feedback_metrics_are_strictly_account_scoped() {
        let (db, schema) = mysql_common::isolated_db("_ops005").await;
        let outcome = tokio::spawn(feedback_account_scope_scenario(db.clone())).await;
        mysql_common::drop_schema(&db, &schema).await;
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(message)) => panic!("OPS-005 feedback scenario must pass: {message}"),
            Err(panic) => std::panic::resume_unwind(panic.into_panic()),
        }
    }

    async fn feedback_account_scope_scenario(db: DatabaseConnection) -> Result<(), String> {
        use personal_secretary::{Clock, SystemClock, VerifiedActorKind};
        use personal_secretary_mysql::build_mysql_inbound_event_store;

        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let managed_a = format!("ops005-a-{suffix}");
        let managed_b = format!("ops005-b-{suffix}");
        let inbound = build_mysql_inbound_event_store(db.clone());
        let now = SystemClock.now_unix_secs();
        let event_a = mysql_common::insert_group_message(
            &inbound,
            &managed_a,
            "ops005-message-a",
            "group-a",
            "member-a",
            VerifiedActorKind::External,
            now,
            "测试消息",
        )
        .await;
        let event_b = mysql_common::insert_group_message(
            &inbound,
            &managed_b,
            "ops005-message-b",
            "group-b",
            "member-b",
            VerifiedActorKind::External,
            now,
            "另一个账号的测试消息",
        )
        .await;

        let account_a = mysql_common::scalar_u64(
            &db,
            "SELECT id AS value FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
            vec![managed_a.clone().into()],
        )
        .await;
        let account_b = mysql_common::scalar_u64(
            &db,
            "SELECT id AS value FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
            vec![managed_b.clone().into()],
        )
        .await;

        for (ordinal, account_id, event_id) in [
            (0_u8, account_a, event_a.as_str()),
            (1, account_b, event_b.as_str()),
            (2, account_b, event_b.as_str()),
        ] {
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO secretary_thread_mutation_proposals (proposal_id, account_id, mutation_kind, proposal_status, impact_json, decision, command_source_event_id, effect_id) VALUES (?, ?, 'split', 'applied', JSON_OBJECT(), 'approve', ?, ?)",
                vec![
                    uuid::Uuid::new_v4().to_string().into(),
                    account_id.into(),
                    event_id.to_owned().into(),
                    format!("ops005-effect-{ordinal}-{suffix}").into(),
                ],
            ))
            .await
            .map_err(|error| error.to_string())?;
        }
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_thread_mutation_proposals (proposal_id, account_id, mutation_kind, proposal_status, impact_json, decision, command_source_event_id) VALUES (?, ?, 'merge', 'applied', JSON_OBJECT(), 'approve', ?), (?, ?, 'split', 'rejected', JSON_OBJECT(), 'reject', ?)",
            vec![
                uuid::Uuid::new_v4().to_string().into(),
                account_a.into(),
                event_a.as_str().into(),
                uuid::Uuid::new_v4().to_string().into(),
                account_a.into(),
                event_a.as_str().into(),
            ],
        ))
        .await
        .map_err(|error| error.to_string())?;

        for (ordinal, account_id, event_id, important) in [
            (0_u8, account_a, event_a.as_str(), false),
            (1, account_a, event_a.as_str(), true),
            (2, account_b, event_b.as_str(), false),
        ] {
            let candidate_id = uuid::Uuid::new_v4().to_string();
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO secretary_notification_candidates (notification_candidate_id, account_id, source_kind, source_id, source_version, match_key_json) VALUES (?, ?, 'agenda', ?, 1, JSON_OBJECT())",
                vec![
                    candidate_id.clone().into(),
                    account_id.into(),
                    uuid::Uuid::new_v4().to_string().into(),
                ],
            ))
            .await
            .map_err(|error| error.to_string())?;
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO secretary_notification_feedback (feedback_id, account_id, notification_candidate_id, important, promote_to_rule, command_source_event_id, audit_summary) VALUES (?, ?, ?, ?, 0, ?, 'owner notification feedback')",
                vec![
                    uuid::Uuid::new_v4().to_string().into(),
                    account_id.into(),
                    candidate_id.into(),
                    important.into(),
                    event_id.to_owned().into(),
                ],
            ))
            .await
            .map_err(|error| format!("feedback {ordinal}: {error}"))?;
        }

        let sample_a = sample_backfill_health(&db, &mysql_common::account(&managed_a))
            .await
            .map_err(|_| "account A health sampling failed".to_owned())?;
        if sample_a.thread_misassociation_feedback != 1
            || sample_a.reminder_false_positive_feedback != 1
        {
            return Err(format!(
                "account A metrics leaked or misclassified: thread={}, reminder={}",
                sample_a.thread_misassociation_feedback, sample_a.reminder_false_positive_feedback
            ));
        }
        let sample_b = sample_backfill_health(&db, &mysql_common::account(&managed_b))
            .await
            .map_err(|_| "account B health sampling failed".to_owned())?;
        if sample_b.thread_misassociation_feedback != 2
            || sample_b.reminder_false_positive_feedback != 1
        {
            return Err("account B metrics did not remain independently scoped".into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct HealthReader {
    receiver: watch::Receiver<HealthSnapshot>,
}

impl HealthReader {
    pub fn latest(&self) -> HealthSnapshot {
        self.receiver.borrow().clone()
    }

    pub async fn changed(&mut self) -> Result<HealthSnapshot, watch::error::RecvError> {
        self.receiver.changed().await?;
        Ok(self.latest())
    }
}

pub struct HealthLogHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl HealthLogHandle {
    pub fn signal_and_detach(self) -> WorkerHandle {
        let _ = self.shutdown.send(true);
        WorkerHandle::new("health_log", self.join)
    }
}

pub fn spawn_health_log_worker(
    aggregator: Arc<HealthAggregator>,
    state: Arc<RuntimeHealthState>,
    db: DatabaseConnection,
    account: SourceAccountRef,
    config: HealthConfig,
) -> (HealthReader, HealthLogHandle) {
    let initial = HealthSnapshot::new(Vec::new(), now_unix());
    let (publisher, receiver) = watch::channel(initial);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let join = tokio::spawn(run_health_worker(
        aggregator,
        state,
        db,
        account,
        config,
        publisher,
        shutdown_receiver,
    ));
    (
        HealthReader { receiver },
        HealthLogHandle { shutdown, join },
    )
}

#[allow(clippy::too_many_arguments)]
async fn run_health_worker(
    aggregator: Arc<HealthAggregator>,
    state: Arc<RuntimeHealthState>,
    db: DatabaseConnection,
    account: SourceAccountRef,
    config: HealthConfig,
    publisher: watch::Sender<HealthSnapshot>,
    mut shutdown: watch::Receiver<bool>,
) {
    info!(
        interval_ms = config.log_interval_ms,
        "B7 健康采样 Worker 已启动"
    );
    loop {
        if *shutdown.borrow() {
            return;
        }
        let now = now_unix();
        match sample_backfill_health(&db, &account).await {
            Ok(sample) => state.mark_database_sample(now as u64, sample),
            Err(()) => state.mark_database_failure(),
        }
        aggregator.invalidate_cache();
        let snapshot = aggregator.snapshot(now).await;
        let _ = publisher.send(snapshot.clone());
        if snapshot.overall_status == HealthStatus::Healthy {
            info!(
                overall = snapshot.overall_status.as_str(),
                subsystem_count = snapshot.subsystems.len(),
                "runtime health snapshot"
            );
        } else {
            warn!(
                overall = snapshot.overall_status.as_str(),
                subsystem_count = snapshot.subsystems.len(),
                "runtime health snapshot"
            );
        }
        for subsystem in &snapshot.subsystems {
            info!(
                subsystem = %subsystem.name,
                status = subsystem.status.as_str(),
                reason_code = subsystem.last_error.as_deref().unwrap_or("none"),
                metrics = ?subsystem.metrics,
                "runtime subsystem health"
            );
        }
        tokio::select! {
            _ = shutdown.changed() => {}
            _ = tokio::time::sleep(Duration::from_millis(config.log_interval_ms.max(1_000))) => {}
        }
    }
}

async fn sample_backfill_health(
    db: &DatabaseConnection,
    account: &SourceAccountRef,
) -> Result<BackfillHealthSample, ()> {
    let account_row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT id
               FROM secretary_accounts
               WHERE source_channel = ? AND platform_account_id = ?
               LIMIT 1"#,
            [
                account.channel.as_str().into(),
                account.account_id.clone().into(),
            ],
        ))
        .await
        .map_err(|_| ())?;
    let Some(account_row) = account_row else {
        return Ok(BackfillHealthSample::default());
    };
    let account_id = account_row.try_get::<u64>("", "id").map_err(|_| ())?;

    let feedback_row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT
                 CAST((SELECT COUNT(*)
                       FROM secretary_thread_mutation_proposals proposal
                       WHERE proposal.account_id = ?
                         AND proposal.mutation_kind = 'split'
                         AND proposal.proposal_status = 'applied'
                         AND proposal.decision = 'approve'
                         AND proposal.command_source_event_id IS NOT NULL) AS SIGNED)
                   AS thread_misassociation_feedback,
                 CAST((SELECT COUNT(*)
                       FROM secretary_notification_feedback feedback
                       WHERE feedback.account_id = ?
                         AND feedback.important = 0) AS SIGNED)
                   AS reminder_false_positive_feedback"#,
            [account_id.into(), account_id.into()],
        ))
        .await
        .map_err(|_| ())?
        .ok_or(())?;

    let gap_row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT
                 CAST(COALESCE(SUM(CASE WHEN status = 'uncertain' THEN 1 ELSE 0 END), 0)
                   AS SIGNED)
                   AS uncertain_gaps,
                 CAST(COALESCE(SUM(CASE WHEN status = 'backfilling' THEN 1 ELSE 0 END), 0)
                   AS SIGNED)
                   AS backfilling_gaps,
                 CAST(COALESCE(SUM(CASE WHEN status = 'unrecoverable' THEN 1 ELSE 0 END), 0)
                   AS SIGNED)
                   AS unrecoverable_gaps
               FROM secretary_ingestion_gaps
               WHERE account_id = ?"#,
            [account_id.into()],
        ))
        .await
        .map_err(|_| ())?
        .ok_or(())?;

    let run_row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT
                 CAST(COALESCE(SUM(CASE WHEN status = 'running' THEN pages_read ELSE 0 END), 0)
                   AS SIGNED)
                   AS pages_read,
                 CAST(COALESCE(SUM(CASE WHEN status = 'running' THEN events_read ELSE 0 END), 0)
                   AS SIGNED)
                   AS events_read,
                 CAST(COALESCE(SUM(CASE WHEN status = 'running' THEN accepted ELSE 0 END), 0)
                   AS SIGNED)
                   AS accepted,
                 CAST(COALESCE(SUM(CASE WHEN status = 'running' THEN duplicates ELSE 0 END), 0)
                   AS SIGNED)
                   AS duplicates,
                 CAST(COALESCE(SUM(CASE WHEN status = 'running' THEN anomaly_count ELSE 0 END), 0)
                   AS SIGNED)
                   AS anomalies,
                 CAST(COALESCE(SUM(CASE WHEN status = 'running' AND budget_exhausted = 1
                                   THEN 1 ELSE 0 END), 0) AS SIGNED)
                   AS budget_exhausted_runs
               FROM secretary_backfill_runs
               WHERE account_id = ?"#,
            [account_id.into()],
        ))
        .await
        .map_err(|_| ())?
        .ok_or(())?;

    let failure_row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT run.failure_class
               FROM secretary_backfill_runs run
               JOIN secretary_ingestion_gaps gap ON gap.gap_id = run.gap_id
               WHERE run.account_id = ?
                 AND run.failure_class IS NOT NULL
                 AND gap.status IN ('uncertain', 'backfilling', 'unrecoverable')
               ORDER BY run.updated_at DESC, run.backfill_run_id DESC
               LIMIT 1"#,
            [account_id.into()],
        ))
        .await
        .map_err(|_| ())?;
    let failure_class = failure_row
        .as_ref()
        .and_then(|row| row.try_get::<String>("", "failure_class").ok());

    let gap_failure_row = if failure_class.is_none() {
        db.query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT reason
               FROM secretary_ingestion_gaps
               WHERE account_id = ? AND status IN ('uncertain', 'backfilling', 'unrecoverable')
               ORDER BY updated_at DESC, gap_id DESC
               LIMIT 1"#,
            [account_id.into()],
        ))
        .await
        .map_err(|_| ())?
    } else {
        None
    };
    let failure_class = failure_class.or_else(|| {
        gap_failure_row
            .as_ref()
            .and_then(|row| row.try_get::<String>("", "reason").ok())
    });

    fn non_negative(row: &sea_orm::QueryResult, column: &str) -> Result<u64, ()> {
        let value = row.try_get::<i64>("", column).map_err(|_| ())?;
        u64::try_from(value).map_err(|_| ())
    }

    Ok(BackfillHealthSample {
        uncertain_gaps: non_negative(&gap_row, "uncertain_gaps")?,
        backfilling_gaps: non_negative(&gap_row, "backfilling_gaps")?,
        unrecoverable_gaps: non_negative(&gap_row, "unrecoverable_gaps")?,
        pages_read: non_negative(&run_row, "pages_read")?,
        events_read: non_negative(&run_row, "events_read")?,
        accepted: non_negative(&run_row, "accepted")?,
        duplicates: non_negative(&run_row, "duplicates")?,
        anomalies: non_negative(&run_row, "anomalies")?,
        budget_exhausted_runs: non_negative(&run_row, "budget_exhausted_runs")?,
        failure_code: failure_class
            .as_deref()
            .map(failure_code_from_text)
            .unwrap_or(FAILURE_NONE),
        thread_misassociation_feedback: non_negative(
            &feedback_row,
            "thread_misassociation_feedback",
        )?,
        reminder_false_positive_feedback: non_negative(
            &feedback_row,
            "reminder_false_positive_feedback",
        )?,
    })
}

fn failure_code_from_text(value: &str) -> u8 {
    match value {
        "queue_overflow" => FAILURE_QUEUE_OVERFLOW,
        "database_unavailable" => FAILURE_DATABASE_UNAVAILABLE,
        "history_unprovable" => FAILURE_HISTORY_UNPROVABLE,
        "invalid_event" => FAILURE_INVALID_EVENT,
        _ => FAILURE_UNKNOWN,
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}
