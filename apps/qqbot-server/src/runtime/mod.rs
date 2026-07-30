//! NapCat 适配器运行入口与 Worker 编排。
//!
//! [`run`] / [`run_with_cancellation`] 只表达启动顺序：基础设施装配 → 可选 Worker
//! 装配 → 连接循环。装配细节下沉到 [`crate::bootstrap`]，连接循环下沉到
//! [`connection_loop`]。所有装配失败路径与关闭路径都回收已启动 Worker。

use std::path::PathBuf;

use personal_secretary::{
    FollowUpUseCase, NotificationPolicyUseCase, SystemClock, build_mysql_follow_up_store,
    build_mysql_memory_store, build_mysql_notification_policy_store,
};
use thiserror::Error;
use tokio::sync::watch;

use crate::bootstrap;
use crate::bootstrap::workers::WorkerHandles;
use crate::config::AppConfig;
use crate::follow_up_worker::spawn_follow_up_worker;
use crate::notification_policy_worker::spawn_notification_policy_worker;
use crate::runtime::connection_loop::run_connection_loop;
use crate::runtime::shutdown::ShutdownSource;

mod connection_loop;
pub(crate) mod handlers;
mod health;
mod shutdown;

pub(crate) use health::BackfillWake;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Database(#[from] sea_orm::DbErr),
    #[error(transparent)]
    Store(#[from] personal_secretary::InboundEventStoreError),
    #[error(transparent)]
    Identity(#[from] personal_secretary::InboundIdentityError),
    #[error("invalid qqbot configuration: {0}")]
    Config(String),
    #[error("QQ Open Platform runtime failed: {0}")]
    OfficialPlatform(String),
    #[error("LLM runtime failed: {0}")]
    Llm(String),
}

/// 生产入口：使用 OS 信号（Ctrl-C / SIGTERM）触发优雅关闭。
pub async fn run(config: AppConfig, config_dir: PathBuf) -> Result<(), RuntimeError> {
    run_with_shutdown(config, config_dir, ShutdownSource::OsSignal).await
}

/// 可编程关闭入口：接收一个 `watch::Receiver<bool>`，当收到 `true` 时触发优雅关闭。
///
/// 用于 E2E 集成测试在不依赖 OS 信号的情况下驱动真实服务并验证关闭。与 [`run`]
/// 共享同一套 Worker 装配和监听器循环，区别只在关闭信号来源。
pub async fn run_with_cancellation(
    config: AppConfig,
    config_dir: PathBuf,
    shutdown: watch::Receiver<bool>,
) -> Result<(), RuntimeError> {
    run_with_shutdown(config, config_dir, ShutdownSource::Watch(shutdown)).await
}

async fn run_with_shutdown(
    config: AppConfig,
    config_dir: PathBuf,
    mut shutdown_source: ShutdownSource,
) -> Result<(), RuntimeError> {
    let infra = bootstrap::infra::assemble_infra(&config, &config_dir).await?;

    let mut handles = WorkerHandles::new();

    if let Err(error) = bootstrap::workers::reconcile_legacy_notification_outbox(
        infra.db.clone(),
        &config.notification_policy,
    )
    .await
    {
        let _ = &error;
        tracing::error!(
            error_code = "legacy_outbox_reconciliation_failed",
            "legacy Owner Outbox 协调失败或存在活跃租约，拒绝启动任何投递相关 Worker"
        );
        return Err(error);
    }

    let notification_policy_use_case = std::sync::Arc::new(NotificationPolicyUseCase::new(
        build_mysql_notification_policy_store(infra.db.clone()),
        std::sync::Arc::new(SystemClock),
    ));
    let follow_up_use_case = std::sync::Arc::new(FollowUpUseCase::new(
        build_mysql_follow_up_store(infra.db.clone()),
        build_mysql_memory_store(infra.db.clone()),
    ));
    if config.follow_up.enabled {
        tracing::info!(
            scan_interval_ms = config.follow_up.scan_interval_ms,
            horizon_secs = config.follow_up.horizon_secs,
            batch_size = config.follow_up.batch_size,
            "结构化记忆维护与承诺提醒调度已启用；仅生成统一策略候选"
        );
        handles.follow_up = Some(spawn_follow_up_worker(
            std::sync::Arc::clone(&follow_up_use_case),
            config.follow_up.clone(),
        ));
    } else {
        tracing::info!("承诺提醒调度已禁用（follow_up.enabled=false）");
    }

    // Agenda 扫描必须在 QQ Open Platform 前启动：它仅入队，实际发送仍由统一 Outbox worker 完成。
    if let Err(error) = bootstrap::agenda::assemble_agenda_notification_worker(
        &mut handles,
        infra.db.clone(),
        &config,
    ) {
        tracing::error!(error = %error, "Agenda 通知扫描装配失败，正在回收已启动的任务");
        let handles_to_clean = std::mem::replace(&mut handles, WorkerHandles::new());
        handles_to_clean.shutdown_all().await;
        return Err(error);
    }

    if config.notification_policy.enabled {
        handles.notification_policy = Some(spawn_notification_policy_worker(
            std::sync::Arc::clone(&notification_policy_use_case),
            config.notification_policy.clone(),
        ));
        tracing::info!(
            worker_id = config.notification_policy.worker_id,
            batch_size = config.notification_policy.batch_size,
            "统一通知策略求值 Worker 已启用"
        );
    } else {
        tracing::info!("统一通知策略求值 Worker 已禁用（notification_policy.enabled=false）");
    }

    // P0 修复：Action Planner 必须在 QQ Open Platform 之前装配，
    // 因为 OwnerCommand 入站时需要 PlannerUseCase 创建 action_run。
    let action_planner_use_case = match bootstrap::action_planner::assemble_action_planner(
        &mut handles,
        infra.db.clone(),
        &config,
    )
    .await
    {
        Ok(use_case) => use_case,
        Err(error) => {
            tracing::error!(error = %error, "Action Planner 装配失败，正在回收已启动的任务");
            // 评审 P1：直接 await 回收，保证 Worker 真正关闭后再返回错误，
            // 不用 spawn 后立即返回（无法保证回收完成）。shutdown_all 有全局 deadline 上限。
            let handles_to_clean = std::mem::replace(&mut handles, WorkerHandles::new());
            handles_to_clean.shutdown_all().await;
            return Err(error);
        }
    };

    if let Err(error) = bootstrap::workers::assemble_official_platform(
        &mut handles,
        &config,
        &infra.db,
        &follow_up_use_case,
        &infra.account,
        &action_planner_use_case,
    )
    .await
    {
        tracing::error!(error = %error, "QQ 开放平台装配失败，正在回收已启动的任务");
        let handles_to_clean = std::mem::replace(&mut handles, WorkerHandles::new());
        handles_to_clean.shutdown_all().await;
        return Err(error);
    }

    // 后续装配（线程投影/语义/关联/回补）可能失败，失败时回收已启动的 Worker。
    if let Err(error) = bootstrap::thread_pipeline::assemble_thread_workers(
        &mut handles,
        infra.db.clone(),
        &config,
        infra.account.clone(),
    )
    .await
    {
        tracing::error!(error = %error, "Worker 装配失败，正在回收已启动的任务");
        let handles_to_clean = std::mem::replace(&mut handles, WorkerHandles::new());
        handles_to_clean.shutdown_all().await;
        return Err(error);
    }

    let backfill_wake = handles
        .backfill
        .as_ref()
        .map(|handle| std::sync::Arc::new(BackfillWake::new(handle.wake_notifier())));

    // B6 Artifact：入站创建 + TTL Worker。
    let artifact_use_case = if config.artifact.enabled {
        let store = personal_secretary::build_mysql_artifact_store(infra.db.clone());
        let use_case = std::sync::Arc::new(personal_secretary::ArtifactUseCase::new(store));
        handles.artifact_ttl = Some(crate::artifact_ttl_worker::spawn_artifact_ttl_worker(
            std::sync::Arc::clone(&use_case),
            config.artifact.clone(),
        ));
        tracing::info!(
            default_ttl_secs = config.artifact.default_ttl_secs,
            ttl_scan_interval_ms = config.artifact.ttl_scan_interval_ms,
            "B6 Artifact 入站与 TTL Worker 已启用"
        );
        Some(use_case)
    } else {
        tracing::info!("B6 Artifact 已禁用（artifact.enabled=false）");
        None
    };

    // B7 健康快照：Recall Spool 的 telemetry 与 WAL 使用同一实例，避免从日志猜测积压。
    let recall_spool_telemetry =
        crate::recall::RecallSpoolTelemetry::new(config.recall_wal.max_bytes);
    let health_state = crate::health_runtime::RuntimeHealthState::new();
    health_state.mark_worker_started();
    if config.health.enabled {
        let aggregator = std::sync::Arc::new(
            crate::health_runtime::build_runtime_health_aggregator_with_recall_spool(
                std::sync::Arc::clone(&health_state),
                std::sync::Arc::clone(&recall_spool_telemetry),
                config.health.cache_ttl_secs,
                config.health.worker_success_stale_secs,
            ),
        );
        let (health_reader, health_handle) = crate::health_runtime::spawn_health_log_worker(
            aggregator,
            std::sync::Arc::clone(&health_state),
            infra.db.clone(),
            infra.account.clone(),
            config.health.clone(),
        );
        handles.health_reader = Some(health_reader);
        handles.health_log = Some(health_handle);
        tracing::info!(
            cache_ttl_secs = config.health.cache_ttl_secs,
            log_interval_ms = config.health.log_interval_ms,
            "B7 健康快照与周期日志已启用"
        );
    } else {
        tracing::info!("B7 健康快照已禁用（health.enabled=false）");
    }

    // B3 撤回闭环：回调先 durable enqueue，Worker 再以 lease 领取并持久化 tombstone。
    let recall_store = personal_secretary::build_mysql_recall_store(infra.db.clone());
    let recall_use_case = std::sync::Arc::new(personal_secretary::RecallUseCase::new(recall_store));
    let (recall_queue, recall_worker) = crate::recall::spawn_recall_worker_with_telemetry(
        std::sync::Arc::clone(&recall_use_case),
        config.recall_wal.clone(),
        std::sync::Arc::clone(&recall_spool_telemetry),
    )
    .map_err(|error| RuntimeError::Config(format!("cannot open recall WAL: {error}")))?;
    handles.recall = Some(recall_worker);
    let recall_handler = std::sync::Arc::new(crate::recall::RecallHandler::new(
        recall_queue,
        infra.account.clone(),
        config.napcat.self_qq_id,
    ));

    run_connection_loop(
        infra.store,
        infra.account,
        &config,
        &mut handles,
        infra.group_whitelist,
        backfill_wake,
        Some(recall_handler),
        Some(std::sync::Arc::clone(&recall_use_case)),
        artifact_use_case,
        config.artifact.default_ttl_secs,
        Some(std::sync::Arc::clone(&health_state)),
        &mut shutdown_source,
    )
    .await
}
