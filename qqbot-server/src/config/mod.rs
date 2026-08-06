//! QQBot 独立配置入口。
//!
//! 只读取应用目录内的 `config/qqbot.toml` 与 `.env`。
//! 所有配置类型通过 `qqbot_server::config::*` 路径暴露，保持外部调用方稳定：
//! TOML 默认值、环境变量优先级、校验语义与序列化字段均与拆分前完全一致。

use std::path::PathBuf;

use thiserror::Error;

// 环境变量覆盖宏与解析助手；`#[macro_use]` 使宏对后续兄弟模块（如 app）可见，
// 且宏内部嵌套调用也能在展开处正确解析。必须先于使用该宏的模块声明。
#[macro_use]
mod env;
mod action_planner;
mod app;
mod database;
mod llm;
mod napcat;
mod qq_open_platform;
mod validation;
mod whitelist;
mod workers;

pub use action_planner::ActionPlannerConfig;
pub use app::AppConfig;
pub use database::DatabaseConfig;
pub use llm::{LlmConfig, LlmProvider, LlmReasoningMode};
pub use napcat::NapCatConfig;
pub use qq_open_platform::QqOpenPlatformConfig;
pub use whitelist::WhitelistConfig;
pub use workers::{
    AgendaConfig, ArtifactConfig, BackfillConfig, DirectorySyncConfig, FollowUpConfig,
    HealthConfig, IngestionConfig, MemoryCandidatesConfig, NotificationPolicyConfig,
    RealtimeSpoolConfig, RecallWalConfig, ReplyReconcileConfig, ThreadLinksConfig,
    ThreadProjectionConfig, ThreadSemanticsConfig,
};

/// 配置加载或校验错误。
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read QQBot config {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse QQBot config {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid QQBot config: {0}")]
    Invalid(String),
}

#[cfg(test)]
mod tests;
