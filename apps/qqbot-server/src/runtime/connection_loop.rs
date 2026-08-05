//! NapCat 正向 WebSocket 连接循环与持久化 Worker 排空。
//!
//! 循环内每次连接建立独立 ConnectionEpoch：先 `begin_connection`，再装配本轮
//! ingestion Worker 与监听器，用 `tokio::select!` 同时监听关闭信号与监听器返回；
//! 连接结束后排空 ingestion Worker、`finish_connection` 产生 Gap 并唤醒回补，
//! 然后按有上限的指数退避无限重连。关闭信号在任何等待点都能抢占。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use personal_secretary::{
    ArtifactUseCase, ConnectionEndReason, ConnectionEpochId, PersonalSecretaryStoreT,
    RecallUseCase, SourceAccountRef,
};
use qqbot::napcat::{NapCatConnectionObserver, NapCatError, NapCatEventHandler, NapCatListener};

use crate::bootstrap::workers::WorkerHandles;
use crate::config::AppConfig;
use crate::inbound::NapCatInboundMapper;
use crate::ingestion_worker::{
    IngestionMetrics, WorkerReport, spawn_ingestion_worker, spawn_spooled_ingestion_worker,
};

use super::RuntimeError;
use super::handlers::{MessageAdmission, PersonalSecretaryInboundHandler};
use super::health::{BackfillWake, ConnectionObserver};
use super::shutdown::ShutdownSource;

/// 运行 NapCat 连接循环，直到收到关闭信号或不可恢复错误。
///
/// 所有装配失败与关闭路径都回收已启动 Worker（`shutdown_all`）。NapCat 重连使用
/// 有上限的指数退避并无限恢复，不会因有限次数退出；关闭信号在退避等待中也能抢占。
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_connection_loop(
    store: Arc<dyn PersonalSecretaryStoreT>,
    account: SourceAccountRef,
    config: &AppConfig,
    handles: &mut WorkerHandles,
    group_whitelist: Arc<HashSet<i64>>,
    backfill_wake: Option<Arc<BackfillWake>>,
    recall_handler: Option<Arc<crate::recall::RecallHandler>>,
    recall_use_case: Option<Arc<RecallUseCase>>,
    artifact_use_case: Option<Arc<ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
    health_state: Option<Arc<crate::health_runtime::RuntimeHealthState>>,
    ingestion_metrics: Arc<IngestionMetrics>,
    realtime_spool: Option<Arc<crate::realtime_spool::RealtimeMessageSpool>>,
    shutdown_source: &mut ShutdownSource,
) -> Result<(), RuntimeError> {
    let mut backoff = config.napcat.reconnect_initial_secs;
    tracing::info!(
        queue_capacity = config.ingestion.queue_capacity,
        retry_initial_ms = config.ingestion.retry_initial_ms,
        retry_max_ms = config.ingestion.retry_max_ms,
        shutdown_drain_timeout_secs = config.ingestion.shutdown_drain_timeout_secs,
        "个人秘书有界持久化队列配置已生效"
    );

    // 评审第三轮 P1-2：能力探测任务句柄纳入生命周期管理。
    // 快速重连时若上一轮探测仍在运行（is_finished=false），先 abort 再发起新探测，
    // 避免重叠探测。关闭路径（shutting_down）也会 abort 并短暂等待探测退出，
    // 不再依赖 detached 任务随 runtime drop 取消。
    let mut probe_handle: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        // Single-flight 去重：上一轮探测未完成则先取消，避免重叠。
        if let Some(handle) = probe_handle.take()
            && !handle.is_finished()
        {
            tracing::warn!("上一轮能力探测仍在运行，已取消以避免重叠");
            handle.abort();
            // 等待 abort 生效，不无限阻塞。
            let _ = tokio::time::timeout(Duration::from_millis(500), handle).await;
        }

        let connection_epoch_id = match store.begin_connection(&account).await {
            Ok(id) => id,
            Err(error) => {
                tracing::error!(error = %error, "begin_connection 失败，正在回收 Worker");
                abort_probe(&mut probe_handle).await;
                let handles_to_shutdown = std::mem::replace(handles, WorkerHandles::new());
                handles_to_shutdown.shutdown_all().await;
                return Err(error.into());
            }
        };

        // B5：连接建立后做一次只读能力探测，建立类型化 capability snapshot。
        // 评审 P0-2：探测**不阻塞**实时 WebSocket 入站。探测在后台任务中并发执行，
        // 受严格整体超时（5s）约束；超时后未完成的能力标记为 Unknown。
        // 探测结果只用于日志与未来健康状态（B7），不影响入站路径。
        // 评审第三轮 P1-2：JoinHandle 保存在 probe_handle，受 single-flight 与关闭管理。
        let napcat_readonly: Arc<dyn qqbot::napcat::NapCatCapabilityReadT> = Arc::new(
            qqbot::napcat::NapCatReadOnlyClient::new(config.napcat.http_base_url.clone()),
        );
        probe_handle = Some(tokio::spawn(async move {
            let snapshot = qqbot::napcat::CapabilitySnapshot::probe(napcat_readonly.as_ref()).await;
            tracing::info!(
                app_name = ?snapshot.app_name,
                app_version = ?snapshot.app_version,
                probe_completed = snapshot.probe_completed,
                heartbeat = snapshot.heartbeat_supported.is_available(),
                online = ?snapshot.online,
                "NapCat 能力探测完成"
            );
        }));

        let (fatal_sender, mut fatal_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (admission, mut ingestion_worker, mut spool_writer) = if let Some(spool) =
            realtime_spool.as_ref()
        {
            let (queue, ingestion_worker) = spawn_spooled_ingestion_worker(
                Arc::clone(&store),
                connection_epoch_id.clone(),
                config.ingestion.clone(),
                recall_use_case.clone(),
                artifact_use_case.clone(),
                artifact_default_ttl_secs,
                health_state.clone().map(|state| {
                    state as Arc<dyn crate::ingestion_worker::IngestionHealthReporterT>
                }),
                Some(Arc::clone(&ingestion_metrics)),
                super::realtime_spool_runtime::checkpoint_adapter(Arc::clone(spool)),
                fatal_sender.clone(),
            );
            let (admission, writer) = super::realtime_spool_runtime::spawn_realtime_spool_writer(
                Arc::clone(spool),
                queue,
                connection_epoch_id.clone(),
                config.realtime_spool.admission_capacity,
                fatal_sender.clone(),
            );
            (
                MessageAdmission::Durable(admission),
                ingestion_worker,
                Some(writer),
            )
        } else {
            let (queue, ingestion_worker) = spawn_ingestion_worker(
                Arc::clone(&store),
                connection_epoch_id.clone(),
                config.ingestion.clone(),
                recall_use_case.clone(),
                artifact_use_case.clone(),
                artifact_default_ttl_secs,
                health_state.clone().map(|state| {
                    state as Arc<dyn crate::ingestion_worker::IngestionHealthReporterT>
                }),
                Some(Arc::clone(&ingestion_metrics)),
            );
            (MessageAdmission::Memory(queue), ingestion_worker, None)
        };
        let handler: Arc<dyn NapCatEventHandler> = Arc::new(PersonalSecretaryInboundHandler {
            mapper: NapCatInboundMapper::new(config.napcat.self_qq_id),
            admission,
            group_whitelist: Arc::clone(&group_whitelist),
            recall_handler: recall_handler.clone(),
        });
        let observer = Arc::new(ConnectionObserver::new(
            Arc::clone(&store),
            connection_epoch_id.clone(),
            backfill_wake.clone(),
            health_state.clone(),
        ));
        let connection_observer: Arc<dyn NapCatConnectionObserver> = observer.clone();
        let listener = NapCatListener::new(
            config.napcat.ws_url.clone(),
            config.napcat.self_qq_id,
            handler,
        )
        .with_connection_observer(connection_observer)
        // 评审第三轮 P1-3：从 QQBot 独立 TOML/env 注入 HeartbeatConfig，
        // 不再固定使用默认值。可按 NapCat 实现调整启动宽限、超时倍数或禁用 watchdog。
        .with_heartbeat_config(config.napcat.heartbeat);

        let (reason, shutting_down, spool_fatal) = tokio::select! {
            _ = shutdown_source.wait() => {
                (ConnectionEndReason::ProcessShutdown, true, None)
            }
            result = listener.run_forward() => {
                match result {
                    Ok(()) => {
                        tracing::warn!("NapCat WebSocket 已断开");
                        (ConnectionEndReason::RemoteClosed, false, None)
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "NapCat WebSocket 运行失败");
                        let reason = match &error {
                            NapCatError::Handler(_) => ConnectionEndReason::ObserverRejected,
                            NapCatError::HeartbeatTimeout(_) => {
                                ConnectionEndReason::HeartbeatTimeout
                            }
                            _ => ConnectionEndReason::TransportError,
                        };
                        (reason, false, None)
                    }
                }
            }
            fatal = fatal_receiver.recv(), if realtime_spool.is_some() => {
                let kind = fatal
                    .map(|fatal| fatal.kind)
                    .unwrap_or(personal_secretary::RealtimeSpoolFatalKind::WriterStopped);
                tracing::error!(
                    error_code = kind.as_str(),
                    "普通消息 durable Spool 进入 fail-closed，结束当前连接周期"
                );
                (ConnectionEndReason::ObserverRejected, false, Some(kind))
            }
        };
        drop(listener);
        let writer_drained = if let Some(writer) = spool_writer.as_mut() {
            drain_spool_writer(
                writer,
                Duration::from_secs(config.realtime_spool.shutdown_drain_timeout_secs),
            )
            .await
        } else {
            true
        };
        let ingestion_drained = drain_ingestion_worker(
            &mut ingestion_worker,
            Duration::from_secs(config.ingestion.shutdown_drain_timeout_secs),
            &connection_epoch_id,
        )
        .await;

        if let Some(kind) = spool_fatal {
            if let Some(spool) = realtime_spool.as_ref() {
                spool.telemetry().set_reconciliation_pending(true);
            }
            let gap_persisted = persist_spool_fatal_gap(
                &store,
                &connection_epoch_id,
                Duration::from_secs(config.realtime_spool.shutdown_drain_timeout_secs),
            )
            .await;
            if gap_persisted && let Some(spool) = realtime_spool.as_ref() {
                spool.telemetry().set_reconciliation_pending(false);
            }
            observer.mark_disconnected();
            abort_probe(&mut probe_handle).await;
            let handles_to_shutdown = std::mem::replace(handles, WorkerHandles::new());
            handles_to_shutdown.shutdown_all().await;
            return Err(RuntimeError::RealtimeSpool(kind.as_str().into()));
        }
        if !writer_drained || !ingestion_drained {
            observer.mark_disconnected();
            abort_probe(&mut probe_handle).await;
            let handles_to_shutdown = std::mem::replace(handles, WorkerHandles::new());
            handles_to_shutdown.shutdown_all().await;
            if shutting_down {
                tracing::warn!(
                    "关闭期限内未完成 durable replay；保留开放 epoch 与 WAL，等待下次启动恢复"
                );
                return Ok(());
            }
            return Err(RuntimeError::RealtimeSpool("shutdown_drain_timeout".into()));
        }

        let gap_id = match store.finish_connection(&connection_epoch_id, reason).await {
            Ok(gap_id) => gap_id,
            Err(error) => {
                tracing::error!(error = %error, "finish_connection 失败，正在回收 Worker");
                abort_probe(&mut probe_handle).await;
                let handles_to_shutdown = std::mem::replace(handles, WorkerHandles::new());
                handles_to_shutdown.shutdown_all().await;
                return Err(error.into());
            }
        };
        if let Some(gap_id) = gap_id {
            tracing::warn!(
                gap_id = %gap_id.as_str(),
                connection_epoch_id = %connection_epoch_id.as_str(),
                reason = reason.as_str(),
                "NapCat 连接结束，已创建待历史回补验证的不确定空窗"
            );
            // 连接结束产生新的 uncertain Gap，唤醒回补 Worker 尽快处理。
            if let Some(wake) = &backfill_wake {
                wake.wake();
            }
            if let Some(health) = &health_state {
                // 只记“存在 uncertain gap”，不把 WS 断开伪装成历史完整。
                health.set_uncertain_gaps(1);
            }
        }
        observer.mark_disconnected();
        if shutting_down {
            tracing::info!("QQBot NapCat 适配器正在退出");
            abort_probe(&mut probe_handle).await;
            let handles_to_shutdown = std::mem::replace(handles, WorkerHandles::new());
            handles_to_shutdown.shutdown_all().await;
            return Ok(());
        }
        if observer.was_connected() {
            backoff = config.napcat.reconnect_initial_secs;
        }

        tracing::info!(backoff_secs = backoff, "等待后重新连接 NapCat");
        tokio::select! {
            _ = shutdown_source.wait() => {
                abort_probe(&mut probe_handle).await;
                let handles_to_shutdown = std::mem::replace(handles, WorkerHandles::new());
                handles_to_shutdown.shutdown_all().await;
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
        }
        backoff = backoff
            .saturating_mul(2)
            .min(config.napcat.reconnect_max_secs);
    }
}

async fn persist_spool_fatal_gap(
    store: &Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: &ConnectionEpochId,
    budget: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    let mut delay = Duration::from_millis(100);
    loop {
        match store
            .mark_connection_uncertain(
                connection_epoch_id,
                personal_secretary::IngestionGapReason::DatabaseUnavailable,
            )
            .await
        {
            Ok(_) => return true,
            Err(error) => {
                let _ = error;
                tracing::warn!(
                    error_code = "gap_persist_failed",
                    retry_delay_ms = delay.as_millis(),
                    "普通消息 Spool fatal Gap 尚未持久化，将对同一 epoch 幂等重试"
                );
            }
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return false;
        }
        tokio::time::sleep(delay.min(deadline.saturating_duration_since(now))).await;
        delay = delay.saturating_mul(2).min(Duration::from_secs(2));
    }
}

pub(super) async fn drain_spool_writer(
    worker: &mut super::realtime_spool_runtime::RealtimeSpoolWriterHandle,
    timeout: Duration,
) -> bool {
    match tokio::time::timeout(timeout, worker.wait()).await {
        Ok(Ok(report)) => {
            tracing::debug!(
                durable_receipts = report.durable_receipts,
                "普通消息 Spool writer 已排空"
            );
            true
        }
        Ok(Err(_)) => {
            tracing::error!(
                error_code = "writer_join_failed",
                "普通消息 Spool writer 异常退出"
            );
            false
        }
        Err(_) => {
            tracing::warn!(
                timeout_ms = timeout.as_millis(),
                "普通消息 Spool writer 未在期限内排空，保留未 checkpoint WAL"
            );
            worker.detach();
            false
        }
    }
}

/// 取消并等待能力探测任务退出（评审第三轮 P1-2）。
/// abort 后用短超时等待 JoinHandle，避免探测任务悬挂但也不无限阻塞关闭路径。
async fn abort_probe(probe_handle: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(handle) = probe_handle.take()
        && !handle.is_finished()
    {
        handle.abort();
        let _ = tokio::time::timeout(Duration::from_millis(500), handle).await;
    }
}

/// 在连接结束前排空持久化 Worker；超时则 abort 并依赖历史回补。
async fn drain_ingestion_worker(
    worker: &mut tokio::task::JoinHandle<WorkerReport>,
    timeout: Duration,
    connection_epoch_id: &ConnectionEpochId,
) -> bool {
    match tokio::time::timeout(timeout, &mut *worker).await {
        Ok(Ok(report)) => {
            tracing::debug!(
                connection_epoch_id = %connection_epoch_id.as_str(),
                accepted = report.accepted,
                duplicates = report.duplicates,
                invalid = report.invalid,
                retries = report.retries,
                dropped = report.dropped,
                "持久化 Worker 已在连接结束前排空"
            );
            true
        }
        Ok(Err(error)) => {
            tracing::error!(
                connection_epoch_id = %connection_epoch_id.as_str(),
                error = %error,
                "持久化 Worker 异常退出；连接空窗将保持 uncertain"
            );
            false
        }
        Err(_) => {
            tracing::warn!(
                connection_epoch_id = %connection_epoch_id.as_str(),
                timeout_ms = timeout.as_millis(),
                "持久化 Worker 未在期限内排空，将中止并依赖历史回补"
            );
            worker.abort();
            let _ = worker.await;
            false
        }
    }
}
