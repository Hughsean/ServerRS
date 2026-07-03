//! Outbox dispatcher (task-book §4.3).
//!
//! Responsibilities ONLY: claim a batch of events, route each to its handler,
//! and mark published / failed / dead based on the handler result. No business
//! logic lives here — that is in `handlers/`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::web_ingestion::event_types::aggregate;
use crate::app::web_ingestion::event_types::event as ev;
use crate::app::web_ingestion::handlers;
use crate::app::web_ingestion::pipeline_context::PipelineContext;
use crate::app::web_ingestion::state_machine_adapter as sm;
use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::{DomainEvent, OutboxClaimQuota, OutboxRepoT};
use crate::domain::web_ingestion::status::{is_terminal_run_status, run_stage, run_status};
use crate::shared::config::WebIngestionHandlerParallelismConfig;
use crate::shared::error::AppError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

/// 主循环中一次 claim-and-spawn 尝试的结果。
enum ClaimLoopResult {
    /// 成功 claim 事件并启动 handler task。
    Spawned,
    /// 已扫描所有 quota，数据库里没有可运行事件。
    NoRunnableEvent,
    /// 有 quota 因 in-flight task 占满 permit 暂时无法继续 claim。
    PermitBusy,
    /// claim 查询失败。
    ClaimError(AppError),
    /// semaphore 已关闭，dispatcher 正在停止。
    ShuttingDown,
}

struct HandlerLimiters {
    default: Arc<Semaphore>,
    crawl_job_created: Arc<Semaphore>,
    url_discovered: Arc<Semaphore>,
    page_fetched: Arc<Semaphore>,
    page_cleaned: Arc<Semaphore>,
    page_distilled: Arc<Semaphore>,
    quality_checked: Arc<Semaphore>,
    document_chunked: Arc<Semaphore>,
    chunks_embedded: Arc<Semaphore>,
    document_indexed: Arc<Semaphore>,
    knowledge_staged: Arc<Semaphore>,
    knowledge_publish_requested: Arc<Semaphore>,
    knowledge_rollback_requested: Arc<Semaphore>,
    terminal: Arc<Semaphore>,
}

impl HandlerLimiters {
    fn new(ctx: &PipelineContext) -> Self {
        let c = &ctx.config.handler_parallelism;
        Self {
            default: semaphore(c.default),
            crawl_job_created: semaphore(c.crawl_job_created),
            url_discovered: semaphore(c.url_discovered),
            page_fetched: semaphore(c.page_fetched),
            page_cleaned: semaphore(c.page_cleaned),
            page_distilled: semaphore(c.page_distilled),
            quality_checked: semaphore(c.quality_checked),
            document_chunked: semaphore(c.document_chunked),
            chunks_embedded: semaphore(c.chunks_embedded),
            document_indexed: semaphore(c.document_indexed),
            knowledge_staged: semaphore(c.knowledge_staged),
            knowledge_publish_requested: semaphore(c.knowledge_publish_requested),
            knowledge_rollback_requested: semaphore(c.knowledge_rollback_requested),
            terminal: semaphore(c.terminal),
        }
    }

    fn limiter_for(&self, event_type: &str) -> Arc<Semaphore> {
        match event_type {
            ev::CRAWL_JOB_CREATED => Arc::clone(&self.crawl_job_created),
            ev::URL_DISCOVERED => Arc::clone(&self.url_discovered),
            ev::PAGE_FETCHED => Arc::clone(&self.page_fetched),
            ev::PAGE_CLEANED => Arc::clone(&self.page_cleaned),
            ev::PAGE_DISTILLED => Arc::clone(&self.page_distilled),
            ev::QUALITY_CHECKED => Arc::clone(&self.quality_checked),
            ev::DOCUMENT_CHUNKED => Arc::clone(&self.document_chunked),
            ev::CHUNKS_EMBEDDED => Arc::clone(&self.chunks_embedded),
            ev::DOCUMENT_INDEXED => Arc::clone(&self.document_indexed),
            ev::KNOWLEDGE_STAGED => Arc::clone(&self.knowledge_staged),
            ev::KNOWLEDGE_PUBLISH_REQUESTED => Arc::clone(&self.knowledge_publish_requested),
            ev::KNOWLEDGE_ROLLBACK_REQUESTED => Arc::clone(&self.knowledge_rollback_requested),
            ev::INGESTION_SKIPPED
            | ev::INGESTION_REJECTED
            | ev::INGESTION_FAILED
            | ev::INGESTION_DEAD
            | ev::KNOWLEDGE_PUBLISHED
            | ev::KNOWLEDGE_SUPERSEDED
            | ev::KNOWLEDGE_ROLLED_BACK => Arc::clone(&self.terminal),
            _ => Arc::clone(&self.default),
        }
    }

    /// Return the Semaphore for a given quota.
    ///
    /// A quota's `event_types` may contain multiple entries (e.g. the terminal
    /// quota groups several terminal events), but they all share one Semaphore.
    /// For the default quota (empty `event_types`), returns the `default` Semaphore.
    fn semaphore_for_quota(&self, quota: &OutboxClaimQuota) -> Arc<Semaphore> {
        if quota.event_types.is_empty() {
            Arc::clone(&self.default)
        } else {
            self.limiter_for(&quota.event_types[0])
        }
    }
}

/// Gracefully drain in-flight tasks after shutdown.
///
/// Waits for all spawned tasks to complete naturally (finalize + heartbeat.stop
/// + permit drop). If the timeout expires, aborts remaining tasks — those events
/// will be recovered by the outbox lock TTL expiring.
async fn graceful_drain_or_abort(join_set: &mut JoinSet<()>, timeout_secs: u64) {
    let drain = async {
        while let Some(joined) = join_set.join_next().await {
            if let Err(e) = joined {
                tracing::error!(
                    error = %e,
                    "web ingestion dispatcher task panicked or was cancelled"
                );
            }
        }
    };

    match tokio::time::timeout(Duration::from_secs(timeout_secs), drain).await {
        Ok(_) => {
            tracing::info!("web ingestion dispatcher drained all in-flight tasks");
        }
        Err(_) => {
            tracing::warn!(
                timeout_secs,
                "web ingestion dispatcher graceful drain timed out; aborting remaining tasks"
            );
            join_set.abort_all();
            while let Some(joined) = join_set.join_next().await {
                if let Err(e) = joined {
                    tracing::debug!(
                        error = %e,
                        "web ingestion dispatcher task aborted"
                    );
                }
            }
        }
    }
}

/// Run the event-driven dispatcher loop.
///
/// Replaces the old `run_tick` polling model. Continuously claims and processes
/// events without waiting for a tick or for a batch to complete. Uses a global
/// Semaphore (`dispatcher_parallelism`) and per-type Semaphores (`handler_parallelism`)
/// to bound concurrency.
///
/// Shutdown: when `shutdown` is cancelled, stops claiming and calls
/// `graceful_drain_or_abort` to wait for in-flight tasks.
pub async fn run(ctx: PipelineContext, shutdown: CancellationToken) {
    let global_sem = Arc::new(Semaphore::new(ctx.config.dispatcher_parallelism.max(1)));
    let limiters = HandlerLimiters::new(&ctx);
    let quotas = build_claim_quotas(&ctx.config.handler_parallelism);
    let mut join_set = JoinSet::new();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!("web ingestion dispatcher shutdown requested");
                break;
            }

            // Continuously drain completed tasks without blocking claim
            joined = join_set.join_next(), if !join_set.is_empty() => {
                if let Some(Err(e)) = joined {
                    tracing::error!(
                        error = %e,
                        "web ingestion dispatcher task panicked or was cancelled"
                    );
                }
                continue;
            }

            global_permit = global_sem.clone().acquire_owned() => {
                let global_permit = match global_permit {
                    Ok(p) => p,
                    Err(_) => break,
                };

                match claim_and_spawn_one(
                    &ctx,
                    &limiters,
                    &quotas,
                    &mut join_set,
                    global_permit,
                ).await {
                    ClaimLoopResult::Spawned => {}
                    ClaimLoopResult::NoRunnableEvent => {
                        tokio::select! {
                            _ = shutdown.cancelled() => break,
                            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                        }
                    }
                    ClaimLoopResult::PermitBusy => {
                        if !wait_after_permit_busy(&shutdown, &mut join_set).await {
                            break;
                        }
                    }
                    ClaimLoopResult::ClaimError(e) => {
                        tracing::warn!(
                            error = %e,
                            "web ingestion dispatcher claim error"
                        );
                        tokio::select! {
                            _ = shutdown.cancelled() => break,
                            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                        }
                    }
                    ClaimLoopResult::ShuttingDown => break,
                }
            }
        }
    }

    graceful_drain_or_abort(
        &mut join_set,
        ctx.config.dispatcher_shutdown_grace_secs.max(1),
    )
    .await;
}

/// Try to claim and spawn exactly one event.
///
/// Iterates quotas in priority order. For each quota:
/// 1. `try_acquire_owned` on the per-type Semaphore (non-blocking).
/// 2. If permit acquired, `claim_one_by_quota` from the DB.
/// 3. If event claimed, spawn a handler task and return `Spawned`.
/// 4. If no event for this quota, drop the permit and `continue` to the next quota.
///
/// Only returns `NoRunnableEvent` after scanning ALL quotas without finding an event.
async fn claim_and_spawn_one(
    ctx: &PipelineContext,
    limiters: &HandlerLimiters,
    quotas: &[OutboxClaimQuota],
    join_set: &mut JoinSet<()>,
    global_permit: OwnedSemaphorePermit,
) -> ClaimLoopResult {
    let lock_ttl_secs = ctx.config.outbox_lock_ttl_secs.max(1);
    let mut permit_busy = false;

    for quota in quotas {
        let sem = limiters.semaphore_for_quota(quota);
        let type_permit = match sem.try_acquire_owned() {
            Ok(permit) => permit,
            Err(TryAcquireError::NoPermits) => {
                permit_busy = true;
                continue;
            }
            Err(TryAcquireError::Closed) => {
                drop(global_permit);
                return ClaimLoopResult::ShuttingDown;
            }
        };

        let claim_token = new_claim_token();

        match ctx
            .outbox_repo
            .claim_one_by_quota(&claim_token, lock_ttl_secs, quota)
            .await
        {
            Ok(Some(event)) => {
                let task_ctx = ctx.clone();
                join_set.spawn(async move {
                    process_claimed_event_with_permits(
                        task_ctx,
                        event,
                        claim_token,
                        global_permit,
                        type_permit,
                    )
                    .await;
                });
                return ClaimLoopResult::Spawned;
            }
            Ok(None) => {
                drop(type_permit);
                continue;
            }
            Err(err) => {
                drop(type_permit);
                drop(global_permit);
                return ClaimLoopResult::ClaimError(AppError::internal(err.to_string()));
            }
        }
    }

    drop(global_permit);
    if permit_busy {
        ClaimLoopResult::PermitBusy
    } else {
        ClaimLoopResult::NoRunnableEvent
    }
}

async fn wait_after_permit_busy(shutdown: &CancellationToken, join_set: &mut JoinSet<()>) -> bool {
    // permit 忙说明容量会随着 in-flight task 完成而恢复，优先等任务完成，避免固定睡眠 500ms。
    if join_set.is_empty() {
        tokio::select! {
            _ = shutdown.cancelled() => false,
            _ = tokio::time::sleep(Duration::from_millis(50)) => true,
        }
    } else {
        tokio::select! {
            _ = shutdown.cancelled() => false,
            joined = join_set.join_next() => {
                if let Some(Err(e)) = joined {
                    tracing::error!(
                        error = %e,
                        "web ingestion dispatcher task panicked or was cancelled"
                    );
                }
                true
            }
        }
    }
}

/// Process a single claimed event. Holds global + type permits until done.
///
/// Compared to the old `process_claimed_event`, the per-type permit is acquired
/// BEFORE claiming (in `claim_and_spawn_one`), so this function does not wait
/// for a limiter — it starts heartbeat and dispatches immediately.
async fn process_claimed_event_with_permits(
    ctx: PipelineContext,
    event: DomainEvent,
    claim_token: String,
    _global_permit: OwnedSemaphorePermit,
    _type_permit: OwnedSemaphorePermit,
) {
    let heartbeat = EventLockHeartbeat::start(
        Arc::clone(&ctx.outbox_repo),
        event.id,
        claim_token.clone(),
        ctx.config.outbox_lock_ttl_secs.max(1),
    );

    let started = Instant::now();
    let result = dispatch(&event, &ctx).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    finalize(&ctx, &event, &claim_token, result, elapsed_ms).await;
    heartbeat.stop().await;

    // drop(global_permit) + drop(type_permit) — automatic on function return
}

fn semaphore(workers: usize) -> Arc<Semaphore> {
    Arc::new(Semaphore::new(workers.max(1)))
}

fn build_claim_quotas(c: &WebIngestionHandlerParallelismConfig) -> Vec<OutboxClaimQuota> {
    let known_event_types = known_event_types();
    vec![
        claim_quota(
            &[ev::KNOWLEDGE_PUBLISH_REQUESTED],
            c.knowledge_publish_requested,
        ),
        claim_quota(
            &[ev::KNOWLEDGE_ROLLBACK_REQUESTED],
            c.knowledge_rollback_requested,
        ),
        claim_quota(&[ev::KNOWLEDGE_STAGED], c.knowledge_staged),
        claim_quota(&[ev::DOCUMENT_INDEXED], c.document_indexed),
        claim_quota(&[ev::CHUNKS_EMBEDDED], c.chunks_embedded),
        claim_quota(&[ev::DOCUMENT_CHUNKED], c.document_chunked),
        claim_quota(&[ev::QUALITY_CHECKED], c.quality_checked),
        claim_quota(&[ev::PAGE_DISTILLED], c.page_distilled),
        claim_quota(&[ev::PAGE_CLEANED], c.page_cleaned),
        claim_quota(
            &[
                ev::INGESTION_SKIPPED,
                ev::INGESTION_REJECTED,
                ev::INGESTION_FAILED,
                ev::INGESTION_DEAD,
                ev::KNOWLEDGE_PUBLISHED,
                ev::KNOWLEDGE_SUPERSEDED,
                ev::KNOWLEDGE_ROLLED_BACK,
            ],
            c.terminal,
        ),
        claim_quota(&[ev::PAGE_FETCHED], c.page_fetched),
        claim_quota(&[ev::URL_DISCOVERED], c.url_discovered),
        claim_quota(&[ev::CRAWL_JOB_CREATED], c.crawl_job_created),
        OutboxClaimQuota {
            event_types: Vec::new(),
            exclude_event_types: known_event_types,
            limit: worker_limit(c.default),
        },
    ]
}

fn claim_quota(event_types: &[&str], workers: usize) -> OutboxClaimQuota {
    OutboxClaimQuota {
        event_types: event_types
            .iter()
            .map(|event_type| (*event_type).to_string())
            .collect(),
        exclude_event_types: Vec::new(),
        limit: worker_limit(workers),
    }
}

fn worker_limit(workers: usize) -> u64 {
    workers.max(1) as u64
}

fn known_event_types() -> Vec<String> {
    [
        ev::CRAWL_JOB_CREATED,
        ev::URL_DISCOVERED,
        ev::PAGE_FETCHED,
        ev::PAGE_CLEANED,
        ev::PAGE_DISTILLED,
        ev::QUALITY_CHECKED,
        ev::DOCUMENT_CHUNKED,
        ev::CHUNKS_EMBEDDED,
        ev::DOCUMENT_INDEXED,
        ev::KNOWLEDGE_STAGED,
        ev::KNOWLEDGE_PUBLISH_REQUESTED,
        ev::KNOWLEDGE_ROLLBACK_REQUESTED,
        ev::INGESTION_SKIPPED,
        ev::INGESTION_REJECTED,
        ev::INGESTION_FAILED,
        ev::INGESTION_DEAD,
        ev::KNOWLEDGE_PUBLISHED,
        ev::KNOWLEDGE_SUPERSEDED,
        ev::KNOWLEDGE_ROLLED_BACK,
    ]
    .iter()
    .map(|event_type| (*event_type).to_string())
    .collect()
}

struct EventLockHeartbeat {
    stop_tx: tokio::sync::oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl EventLockHeartbeat {
    fn start(
        outbox_repo: Arc<dyn OutboxRepoT>,
        event_id: u64,
        claim_token: String,
        lock_ttl_secs: u32,
    ) -> Self {
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
        let heartbeat_secs = (lock_ttl_secs as u64 / 2).max(1);
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(heartbeat_secs));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match outbox_repo
                            .renew_lock(event_id, &claim_token, lock_ttl_secs.max(1))
                            .await
                        {
                            Ok(true) => {}
                            Ok(false) => {
                                tracing::trace!(
                                    event_id,
                                    "web ingestion event lock heartbeat skipped: lock no longer owned"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    event_id,
                                    error = %e,
                                    "web ingestion event lock heartbeat failed"
                                );
                            }
                        }
                    }
                    _ = &mut stop_rx => break,
                }
            }
        });
        Self { stop_tx, task }
    }

    async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.task.await;
    }
}

/// Route an event to its handler. Unknown types → Err (never marked published).
async fn dispatch(event: &DomainEvent, ctx: &PipelineContext) -> Result<(), WebIngestionError> {
    match event.event_type.as_str() {
        ev::CRAWL_JOB_CREATED => handlers::crawl_job_created::handle(event, ctx).await,
        ev::URL_DISCOVERED => handlers::url_discovered::handle(event, ctx).await,
        ev::PAGE_FETCHED => handlers::page_fetched::handle(event, ctx).await,
        ev::PAGE_CLEANED => handlers::page_cleaned::handle(event, ctx).await,
        ev::PAGE_DISTILLED => handlers::page_distilled::handle(event, ctx).await,
        ev::QUALITY_CHECKED => handlers::quality_checked::handle(event, ctx).await,
        ev::DOCUMENT_CHUNKED => handlers::document_chunked::handle(event, ctx).await,
        ev::CHUNKS_EMBEDDED => handlers::chunks_embedded::handle(event, ctx).await,
        ev::DOCUMENT_INDEXED => handlers::document_indexed::handle(event, ctx).await,
        ev::KNOWLEDGE_STAGED => handlers::knowledge_staged::handle(event, ctx).await,
        ev::KNOWLEDGE_PUBLISH_REQUESTED => handlers::publish_requested::handle(event, ctx).await,
        ev::KNOWLEDGE_ROLLBACK_REQUESTED => handlers::rollback_requested::handle(event, ctx).await,
        // Terminal events — no-op, mark published.
        ev::INGESTION_SKIPPED
        | ev::INGESTION_REJECTED
        | ev::INGESTION_FAILED
        | ev::INGESTION_DEAD
        | ev::KNOWLEDGE_PUBLISHED
        | ev::KNOWLEDGE_SUPERSEDED
        | ev::KNOWLEDGE_ROLLED_BACK => handlers::terminal::handle(event).await,
        other => Err(WebIngestionError::Internal(format!(
            "unsupported outbox event type: {other}"
        ))),
    }
}

/// Apply the handler outcome to the outbox row (claim-token guarded).
async fn finalize(
    ctx: &PipelineContext,
    event: &DomainEvent,
    claim_token: &str,
    result: Result<(), WebIngestionError>,
    elapsed_ms: u64,
) {
    match result {
        Ok(()) => match ctx.outbox_repo.mark_published(event.id, claim_token).await {
            Ok(true) => {
                tracing::trace!(
                    event_id = event.id,
                    event_type = %event.event_type,
                    aggregate_id = event.aggregate_id,
                    elapsed_ms,
                    "web ingestion event marked published"
                );
            }
            Ok(false) => tracing::warn!(
                event_id = event.id,
                "mark_published: 0 rows — lock stolen or already processed"
            ),
            Err(e) => tracing::error!(event_id = event.id, error = %e, "mark_published failed"),
        },
        Err(e) => {
            let is_dead = event.retry_count + 1 >= event.max_retries;
            let exponential_delay = ctx
                .config
                .retry_base_delay_secs
                .saturating_mul(2u64.saturating_pow(event.retry_count))
                .min(ctx.config.retry_max_delay_secs);
            let delay = e
                .retry_after_secs()
                .map(|retry_after| retry_after.max(exponential_delay))
                .unwrap_or(exponential_delay);
            let next_retry = chrono::Utc::now() + chrono::Duration::seconds(delay as i64);
            match ctx
                .outbox_repo
                .mark_failed_or_dead(event.id, claim_token, &e.to_string(), next_retry, is_dead)
                .await
            {
                Ok(true) => {
                    if is_dead {
                        mark_run_dead(ctx, event, &e.to_string()).await;
                    }
                    tracing::warn!(
                        event_id = event.id, event_type = %event.event_type, is_dead,
                        retry_delay_secs = delay, elapsed_ms, error = %e, "event handler failed"
                    );
                }
                Ok(false) => tracing::warn!(
                    event_id = event.id,
                    "mark_failed_or_dead: 0 rows — lock stolen or already processed"
                ),
                Err(mark_err) => tracing::error!(
                    event_id = event.id, handler_error = %e, mark_error = %mark_err,
                    "CRITICAL: handler failed AND mark_failed_or_dead failed — event stuck"
                ),
            }
        }
    }
}

async fn mark_run_dead(ctx: &PipelineContext, event: &DomainEvent, error: &str) {
    if event.aggregate_type != aggregate::KNOWLEDGE_INGESTION_RUN {
        return;
    }

    let run = match ctx.run_repo.find_by_id(event.aggregate_id).await {
        Ok(Some(run)) => run,
        Ok(None) => return,
        Err(e) => {
            tracing::error!(
                run_id = event.aggregate_id,
                error = %e,
                "failed to load ingestion run while marking dead"
            );
            return;
        }
    };
    if is_terminal_run_status(&run.status) {
        return;
    }

    match sm::transition(
        &ctx.run_repo,
        run.id,
        &run.status,
        &run.stage,
        run_status::DEAD,
        run_stage::DEAD,
        Some(error),
    )
    .await
    {
        Ok(outcome) if outcome.applied() => {
            if let Err(e) = ctx.run_repo.mark_finished(run.id).await {
                tracing::error!(run_id = run.id, error = %e, "failed to set run finished_at");
            }
        }
        Ok(_) => {}
        Err(e) => tracing::error!(run_id = run.id, error = %e, "failed to mark ingestion run dead"),
    }
}

/// Convenience: build a claim token (exposed for tests).
pub fn new_claim_token() -> String {
    format!("dispatcher:{}", uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota_for<'a>(quotas: &'a [OutboxClaimQuota], event_type: &str) -> &'a OutboxClaimQuota {
        quotas
            .iter()
            .find(|quota| quota.event_types.iter().any(|value| value == event_type))
            .expect("quota for event type")
    }

    #[test]
    fn claim_quotas_follow_handler_parallelism() {
        let mut config = WebIngestionHandlerParallelismConfig::default();
        config.page_cleaned = 2;
        config.chunks_embedded = 3;
        config.url_discovered = 4;

        let quotas = build_claim_quotas(&config);

        assert_eq!(quota_for(&quotas, ev::PAGE_CLEANED).limit, 2);
        assert_eq!(quota_for(&quotas, ev::CHUNKS_EMBEDDED).limit, 3);
        assert_eq!(quota_for(&quotas, ev::URL_DISCOVERED).limit, 4);
    }

    #[test]
    fn claim_quotas_prefer_late_pipeline_without_overclaiming_stage() {
        let mut config = WebIngestionHandlerParallelismConfig::default();
        config.document_indexed = 1;
        config.page_cleaned = 2;
        config.page_fetched = 6;

        let quotas = build_claim_quotas(&config);
        let document_indexed_pos = quotas
            .iter()
            .position(|quota| {
                quota
                    .event_types
                    .contains(&ev::DOCUMENT_INDEXED.to_string())
            })
            .unwrap();
        let page_cleaned_pos = quotas
            .iter()
            .position(|quota| quota.event_types.contains(&ev::PAGE_CLEANED.to_string()))
            .unwrap();
        let page_fetched_pos = quotas
            .iter()
            .position(|quota| quota.event_types.contains(&ev::PAGE_FETCHED.to_string()))
            .unwrap();

        assert!(document_indexed_pos < page_cleaned_pos);
        assert!(page_cleaned_pos < page_fetched_pos);
        assert_eq!(quota_for(&quotas, ev::PAGE_CLEANED).limit, 2);
    }

    #[test]
    fn default_quota_excludes_known_events() {
        let config = WebIngestionHandlerParallelismConfig::default();
        let quotas = build_claim_quotas(&config);
        let default_quota = quotas.last().expect("default quota");

        assert!(default_quota.event_types.is_empty());
        assert!(
            default_quota
                .exclude_event_types
                .contains(&ev::PAGE_CLEANED.to_string())
        );
        assert!(
            default_quota
                .exclude_event_types
                .contains(&ev::CRAWL_JOB_CREATED.to_string())
        );
    }

    #[tokio::test]
    async fn permit_busy_wait_wakes_when_in_flight_task_finishes() {
        let shutdown = CancellationToken::new();
        let mut join_set = JoinSet::new();
        join_set.spawn(async {
            tokio::time::sleep(Duration::from_millis(20)).await;
        });

        let started = Instant::now();
        let should_continue = wait_after_permit_busy(&shutdown, &mut join_set).await;

        assert!(should_continue);
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "permit 忙时应该在任务完成后唤醒，而不是固定等待 500ms"
        );
        assert!(join_set.is_empty());
    }
}
