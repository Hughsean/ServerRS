//! Worker 句柄聚合与 QQ 开放平台装配。
//!
//! `WorkerHandles` 在装配过程中逐步累积已启动的 Worker；任一后续步骤失败或运行结束，
//! 调用 [`WorkerHandles::shutdown_all`] 用单一全局 deadline 并发回收，避免顺序等待
//! （极端总关闭 N×deadline）与超时后未 abort 的问题。

use std::sync::Arc;

use personal_secretary::{FollowUpUseCase, PlannerUseCase, build_mysql_inbound_event_store};
use sea_orm::DatabaseConnection;

use crate::action_planner_worker::ActionPlannerHandle;
use crate::artifact_ttl_worker::ArtifactTtlHandle;
use crate::backfill::BackfillHandle;
use crate::config::AppConfig;
use crate::directory_sync::DirectorySyncHandle;
use crate::follow_up_worker::FollowUpHandle;
use crate::health_runtime::{HealthLogHandle, HealthReader};
use crate::qq_open_platform::{OfficialPlatformHandle, spawn_official_platform};
use crate::recall::RecallWorkerHandle;
use crate::runtime::RuntimeError;
use crate::thread_links::ThreadLinksHandle;
use crate::thread_projection::ThreadProjectionHandle;
use crate::thread_semantics::ThreadSemanticsHandle;
use crate::worker_lifecycle::RuntimeWorkers;

/// 全局关闭期限：所有 Worker 并发关闭的总上限。
/// backfill 内部有 10s `SHUTDOWN_GRACE`，外层必须留出额外余量，否则会抢先中止内部清理。
const WORKER_SHUTDOWN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(25);

/// 聚合所有可选 Worker 句柄，支持并发优雅关闭。
///
/// 装配过程中逐步 push 已启动的 Worker；若后续步骤失败，调用 [`WorkerHandles::shutdown_all`]
/// 回收已启动的任务，避免资源泄漏。正常运行结束时同样调用该方法。
pub(crate) struct WorkerHandles {
    pub(crate) backfill: Option<BackfillHandle>,
    pub(crate) thread_projection: Option<ThreadProjectionHandle>,
    pub(crate) thread_semantics: Option<ThreadSemanticsHandle>,
    pub(crate) thread_links: Option<ThreadLinksHandle>,
    pub(crate) follow_up: Option<FollowUpHandle>,
    pub(crate) official_platform: Option<OfficialPlatformHandle>,
    pub(crate) action_planner: Option<ActionPlannerHandle>,
    pub(crate) directory_sync: Option<DirectorySyncHandle>,
    pub(crate) artifact_ttl: Option<ArtifactTtlHandle>,
    pub(crate) recall: Option<RecallWorkerHandle>,
    pub(crate) health_reader: Option<HealthReader>,
    pub(crate) health_log: Option<HealthLogHandle>,
}

impl WorkerHandles {
    pub(crate) fn new() -> Self {
        Self {
            backfill: None,
            thread_projection: None,
            thread_semantics: None,
            thread_links: None,
            follow_up: None,
            official_platform: None,
            action_planner: None,
            directory_sync: None,
            artifact_ttl: None,
            recall: None,
            health_reader: None,
            health_log: None,
        }
    }

    /// 取出所有句柄，发出停止信号并用单一全局 deadline 并发回收。
    pub(crate) async fn shutdown_all(self) {
        let mut workers = RuntimeWorkers::new();
        if let Some(handle) = self.backfill {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.thread_projection {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.thread_semantics {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.thread_links {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.follow_up {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.official_platform {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.action_planner {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.directory_sync {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.artifact_ttl {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.recall {
            workers.push(handle.signal_and_detach());
        }
        if let Some(handle) = self.health_log {
            workers.push(handle.signal_and_detach());
        }
        workers.shutdown_all(WORKER_SHUTDOWN_DEADLINE).await;
    }
}

/// 装配 QQ 开放平台通道。必须在 Action Planner 之后、线程派生 Worker 之前装配。
///
/// 返回 `Ok(())` 表示已启动或已禁用；失败由调用方回收此前已启动的 Worker。
pub(crate) async fn assemble_official_platform(
    handles: &mut WorkerHandles,
    config: &AppConfig,
    db: &DatabaseConnection,
    follow_up_use_case: &Arc<FollowUpUseCase>,
    account: &personal_secretary::SourceAccountRef,
    action_planner_use_case: &Option<Arc<PlannerUseCase>>,
) -> Result<(), RuntimeError> {
    if !config.qq_open_platform.enabled {
        tracing::info!("QQ Open Platform 已禁用（qq_open_platform.enabled=false）");
        return Ok(());
    }
    let inbound = build_mysql_inbound_event_store(db.clone());
    let official_handle = spawn_official_platform(
        config.qq_open_platform.clone(),
        db.clone(),
        inbound,
        Arc::clone(follow_up_use_case),
        account.clone(),
        action_planner_use_case.clone(),
    )
    .await
    .map_err(|error| RuntimeError::OfficialPlatform(error.to_string()))?;
    handles.official_platform = Some(official_handle);
    tracing::info!(
        app_id = config.qq_open_platform.app_id,
        "QQ Open Platform Gateway 与 Owner-only Outbox 投递已启动"
    );
    Ok(())
}
