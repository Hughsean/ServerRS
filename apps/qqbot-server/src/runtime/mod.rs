//! NapCat 适配器运行入口与 Worker 编排。
//!
//! [`run`] / [`run_with_cancellation`] 只表达启动顺序：基础设施装配 → 可选 Worker
//! 装配 → 连接循环。装配细节下沉到 [`crate::bootstrap`]，连接循环下沉到
//! [`connection_loop`]。所有装配失败路径与关闭路径都回收已启动 Worker。

use std::path::PathBuf;

use personal_secretary::{FollowUpUseCase, build_mysql_follow_up_store, build_mysql_memory_store};
use thiserror::Error;
use tokio::sync::watch;

use crate::bootstrap;
use crate::bootstrap::workers::WorkerHandles;
use crate::config::AppConfig;
use crate::follow_up_worker::spawn_follow_up_worker;
use crate::runtime::connection_loop::run_connection_loop;
use crate::runtime::shutdown::ShutdownSource;

mod connection_loop;
mod handlers;
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

    let follow_up_use_case = std::sync::Arc::new(FollowUpUseCase::new(
        build_mysql_follow_up_store(infra.db.clone()),
        build_mysql_memory_store(infra.db.clone()),
    ));
    if config.follow_up.enabled {
        tracing::info!(
            scan_interval_ms = config.follow_up.scan_interval_ms,
            horizon_secs = config.follow_up.horizon_secs,
            batch_size = config.follow_up.batch_size,
            "结构化记忆维护与承诺提醒调度已启用；通知仅写入 Outbox"
        );
        handles.follow_up = Some(spawn_follow_up_worker(
            std::sync::Arc::clone(&follow_up_use_case),
            config.follow_up.clone(),
        ));
    } else {
        tracing::info!("承诺提醒调度已禁用（follow_up.enabled=false）");
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

    run_connection_loop(
        infra.store,
        infra.account,
        &config,
        &mut handles,
        infra.group_whitelist,
        backfill_wake,
        &mut shutdown_source,
    )
    .await
}
