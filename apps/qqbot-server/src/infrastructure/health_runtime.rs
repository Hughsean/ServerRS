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

#[derive(Debug, Default)]
pub struct RuntimeHealthState {
    websocket_observed: AtomicBool,
    websocket_connected: AtomicBool,
    history_observed: AtomicBool,
    uncertain_gaps: AtomicU64,
    database_state: AtomicU8,
    workers_state: AtomicU8,
    worker_started: AtomicBool,
    last_database_success_unix: AtomicU64,
    last_worker_success_unix: AtomicU64,
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

    fn mark_database_sample(&self, now_unix: u64, uncertain_gaps: u64) {
        self.database_state.store(HEALTHY, Ordering::Release);
        self.last_database_success_unix
            .store(now_unix, Ordering::Release);
        self.set_uncertain_gaps(uncertain_gaps);
    }

    fn mark_database_failure(&self) {
        self.database_state.store(DEGRADED, Ordering::Release);
        self.history_observed.store(false, Ordering::Release);
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
struct IngestionMetricsProducer(Arc<crate::ingestion_worker::IngestionMetrics>);
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
        let count = self.0.uncertain_gaps.load(Ordering::Acquire);
        SubsystemHealth {
            name: self.name().into(),
            status: if !observed {
                HealthStatus::Uncertain
            } else if count == 0 {
                HealthStatus::Healthy
            } else {
                HealthStatus::Degraded
            },
            last_success_at_unix_secs: None,
            last_error: (!observed)
                .then(|| "not_observed".into())
                .or_else(|| (count > 0).then(|| "uncertain_gaps_present".into())),
            metrics: BTreeMap::new(),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
        match sample_uncertain_gaps(&db, &account).await {
            Ok(count) => state.mark_database_sample(now as u64, count),
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

async fn sample_uncertain_gaps(
    db: &DatabaseConnection,
    account: &SourceAccountRef,
) -> Result<u64, ()> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT COUNT(*) AS value
               FROM secretary_ingestion_gaps gap
               JOIN secretary_accounts account ON account.id = gap.account_id
               WHERE account.source_channel = ? AND account.platform_account_id = ?
                 AND gap.status IN ('uncertain', 'backfilling', 'unrecoverable')"#,
            [
                account.channel.as_str().into(),
                account.account_id.clone().into(),
            ],
        ))
        .await
        .map_err(|_| ())?
        .ok_or(())?;
    let value = row.try_get::<i64>("", "value").map_err(|_| ())?;
    u64::try_from(value).map_err(|_| ())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}
