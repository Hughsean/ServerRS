//! QQBot 服务库。
//!
//! 对外稳定入口是 [`config`] 与 [`runtime`]。
//! [`production`] 暴露可组合的生产装配边界，供运行时与机器验收共用；
//! 验收测试必须经过这些入口，禁止用 Fake 冒充 L4/L5。

#[path = "adapters/action_planner.rs"]
mod action_planner;
#[path = "application/action_planner_worker.rs"]
mod action_planner_worker;
#[path = "application/agenda_notification_worker.rs"]
mod agenda_notification_worker;
#[path = "application/artifact_ttl_worker.rs"]
mod artifact_ttl_worker;
#[path = "application/backfill/mod.rs"]
mod backfill;
mod bootstrap;
pub mod config;
#[path = "application/directory_sync.rs"]
mod directory_sync;
#[path = "application/follow_up_worker.rs"]
mod follow_up_worker;
#[path = "infrastructure/health_runtime.rs"]
mod health_runtime;
#[path = "adapters/inbound.rs"]
mod inbound;
#[path = "application/ingestion_worker.rs"]
mod ingestion_worker;
#[path = "infrastructure/llm.rs"]
mod llm;
#[path = "application/memory_candidates.rs"]
mod memory_candidates;
#[path = "adapters/napcat_directory.rs"]
mod napcat_directory;
#[path = "adapters/napcat_history_source.rs"]
mod napcat_history_source;
#[path = "application/notification_policy_worker.rs"]
mod notification_policy_worker;
#[path = "adapters/owner_approval.rs"]
pub mod owner_approval;
#[path = "adapters/qq_open_platform.rs"]
mod qq_open_platform;
#[path = "infrastructure/qq_open_platform_mysql.rs"]
mod qq_open_platform_mysql;
#[path = "infrastructure/recall.rs"]
mod recall;
#[path = "application/reply_reconcile_worker.rs"]
mod reply_reconcile_worker;
pub mod runtime;
#[path = "application/thread_links.rs"]
mod thread_links;
#[path = "application/thread_projection.rs"]
mod thread_projection;
#[path = "application/thread_semantics.rs"]
mod thread_semantics;
#[path = "application/worker_lifecycle.rs"]
pub mod worker_lifecycle;

/// 生产装配边界：消息入站、撤回队列、目录同步、Artifact TTL、健康快照。
///
/// 这些符号是真实生产路径的一部分，不是测试替身。
pub mod production {
    pub use crate::artifact_ttl_worker::{ArtifactTtlHandle, spawn_artifact_ttl_worker};
    pub use crate::directory_sync::spawn_directory_sync_worker;
    pub use crate::health_runtime::{
        HealthLogHandle, HealthReader, RuntimeHealthState, build_runtime_health_aggregator,
        build_runtime_health_aggregator_with_recall_spool, spawn_health_log_worker,
    };
    pub use crate::inbound::NapCatInboundMapper;
    pub use crate::ingestion_worker::{
        IngestionEnqueueError, IngestionHealthReporterT, IngestionQueue, WorkerReport,
        spawn_ingestion_worker,
    };
    pub use crate::napcat_directory::NapCatDirectorySource;
    pub use crate::recall::{
        RecallHandler, RecallQueue, RecallSpoolSnapshot, RecallSpoolTelemetry, spawn_recall_worker,
        spawn_recall_worker_with_telemetry,
    };
    pub use crate::runtime::handlers::PersonalSecretaryInboundHandler;
    pub use crate::worker_lifecycle::WorkerHandle;
}
