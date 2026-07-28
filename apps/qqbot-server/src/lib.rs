//! QQBot 服务库。
//!
//! 对外稳定入口是 [`config`] 与 [`runtime`]。
//! [`production`] 暴露可组合的生产装配边界，供运行时与机器验收共用；
//! 验收测试必须经过这些入口，禁止用 Fake 冒充 L4/L5。

mod action_planner;
mod action_planner_worker;
mod agenda_notification_worker;
mod artifact_ttl_worker;
mod backfill;
mod bootstrap;
pub mod config;
mod directory_sync;
mod follow_up_worker;
mod health_runtime;
mod inbound;
mod ingestion_worker;
mod llm;
pub mod owner_approval;
mod qq_open_platform;
mod recall;
pub mod runtime;
mod thread_links;
mod thread_projection;
mod thread_semantics;
pub mod worker_lifecycle;

/// 生产装配边界：消息入站、撤回队列、目录同步、Artifact TTL、健康快照。
///
/// 这些符号是真实生产路径的一部分，不是测试替身。
pub mod production {
    pub use crate::artifact_ttl_worker::{ArtifactTtlHandle, spawn_artifact_ttl_worker};
    pub use crate::directory_sync::{NapCatDirectorySource, spawn_directory_sync_worker};
    pub use crate::health_runtime::{
        HealthLogHandle, HealthReader, RuntimeHealthState, build_runtime_health_aggregator,
        build_runtime_health_aggregator_with_recall_spool, spawn_health_log_worker,
    };
    pub use crate::inbound::NapCatInboundMapper;
    pub use crate::ingestion_worker::{IngestionQueue, WorkerReport, spawn_ingestion_worker};
    pub use crate::recall::{
        RecallHandler, RecallQueue, RecallSpoolSnapshot, RecallSpoolTelemetry, spawn_recall_worker,
        spawn_recall_worker_with_telemetry,
    };
    pub use crate::runtime::handlers::PersonalSecretaryInboundHandler;
    pub use crate::worker_lifecycle::WorkerHandle;
}
