//! 后台扫描与派生 Worker 的有界配置集合。
//!
//! 包含入站队列、历史回补、线程投影、线程语义、线程关联与跟进提醒。
//! 所有配置只在 QQBot 应用目录内生效，不读取数字人配置；预算业务不变量集中在领域层。

use personal_secretary::BackfillBudget;
use serde::Deserialize;

use super::ConfigError;

/// 入站消息队列与重试配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestionConfig {
    #[serde(default = "default_ingestion_queue_capacity")]
    pub queue_capacity: usize,
    #[serde(default = "default_ingestion_retry_initial_ms")]
    pub retry_initial_ms: u64,
    #[serde(default = "default_ingestion_retry_max_ms")]
    pub retry_max_ms: u64,
    #[serde(default = "default_ingestion_shutdown_drain_timeout_secs")]
    pub shutdown_drain_timeout_secs: u64,
}

impl Default for IngestionConfig {
    fn default() -> Self {
        Self {
            queue_capacity: default_ingestion_queue_capacity(),
            retry_initial_ms: default_ingestion_retry_initial_ms(),
            retry_max_ms: default_ingestion_retry_max_ms(),
            shutdown_drain_timeout_secs: default_ingestion_shutdown_drain_timeout_secs(),
        }
    }
}

fn default_ingestion_queue_capacity() -> usize {
    1_024
}

fn default_ingestion_retry_initial_ms() -> u64 {
    100
}

fn default_ingestion_retry_max_ms() -> u64 {
    5_000
}

fn default_ingestion_shutdown_drain_timeout_secs() -> u64 {
    10
}

/// 历史回补配置。所有历史读取必须有明确上限，禁止无限循环或一次加载全部历史。
/// 仅属于 `apps/qqbot-server`，不读取数字人的配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BackfillConfig {
    pub enabled: bool,
    pub page_size: u32,
    pub max_pages_per_scope: u32,
    pub max_events_per_run: u32,
    pub max_concurrency: u32,
    pub lease_secs: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
}

impl Default for BackfillConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            page_size: 100,
            max_pages_per_scope: 20,
            max_events_per_run: 2000,
            max_concurrency: 2,
            lease_secs: 60,
            retry_initial_ms: 500,
            retry_max_ms: 10_000,
        }
    }
}

impl BackfillConfig {
    /// 构造领域层有界预算并校验。配置层只负责填充与校验，业务不变量集中在领域层。
    pub fn budget(&self) -> Result<BackfillBudget, ConfigError> {
        self.validate_budget()?;
        Ok(BackfillBudget {
            page_size: self.page_size,
            max_pages_per_scope: self.max_pages_per_scope,
            max_events_per_run: self.max_events_per_run,
            max_concurrency: self.max_concurrency,
            lease_secs: self.lease_secs,
            retry_initial_ms: self.retry_initial_ms,
            retry_max_ms: self.retry_max_ms,
        })
    }

    fn validate_budget(&self) -> Result<(), ConfigError> {
        if self.page_size == 0 || self.page_size > 100 {
            return Err(ConfigError::Invalid(
                "backfill.page_size must be between 1 and 100".into(),
            ));
        }
        if self.max_pages_per_scope == 0 {
            return Err(ConfigError::Invalid(
                "backfill.max_pages_per_scope must be positive".into(),
            ));
        }
        if self.max_events_per_run == 0 {
            return Err(ConfigError::Invalid(
                "backfill.max_events_per_run must be positive".into(),
            ));
        }
        if self.max_concurrency == 0 || self.max_concurrency > 64 {
            return Err(ConfigError::Invalid(
                "backfill.max_concurrency must be between 1 and 64".into(),
            ));
        }
        if self.lease_secs == 0 || self.lease_secs > 3600 {
            return Err(ConfigError::Invalid(
                "backfill.lease_secs must be between 1 and 3600".into(),
            ));
        }
        if self.retry_initial_ms == 0 {
            return Err(ConfigError::Invalid(
                "backfill.retry_initial_ms must be positive".into(),
            ));
        }
        if self.retry_max_ms < self.retry_initial_ms {
            return Err(ConfigError::Invalid(
                "backfill.retry_max_ms must be >= retry_initial_ms".into(),
            ));
        }
        Ok(())
    }
}

/// 确定性事件线程投影配置。只消费已持久化事件，不调用 LLM。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ThreadProjectionConfig {
    pub enabled: bool,
    pub batch_size: u32,
    pub max_batches_per_scan: u32,
    pub same_conversation_window_secs: i64,
    pub lease_secs: u64,
    pub scan_interval_ms: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
}

impl Default for ThreadProjectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            batch_size: 100,
            max_batches_per_scan: 10,
            same_conversation_window_secs: 300,
            lease_secs: 60,
            scan_interval_ms: 500,
            retry_initial_ms: 500,
            retry_max_ms: 10_000,
        }
    }
}

impl ThreadProjectionConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.batch_size == 0 || self.batch_size > 1000 {
            return Err(ConfigError::Invalid(
                "thread_projection.batch_size must be between 1 and 1000".into(),
            ));
        }
        if self.max_batches_per_scan == 0 || self.max_batches_per_scan > 100 {
            return Err(ConfigError::Invalid(
                "thread_projection.max_batches_per_scan must be between 1 and 100".into(),
            ));
        }
        if self.same_conversation_window_secs <= 0 || self.same_conversation_window_secs > 86_400 {
            return Err(ConfigError::Invalid(
                "thread_projection.same_conversation_window_secs must be between 1 and 86400"
                    .into(),
            ));
        }
        if self.lease_secs == 0 || self.lease_secs > 3600 {
            return Err(ConfigError::Invalid(
                "thread_projection.lease_secs must be between 1 and 3600".into(),
            ));
        }
        if self.scan_interval_ms == 0 {
            return Err(ConfigError::Invalid(
                "thread_projection.scan_interval_ms must be positive".into(),
            ));
        }
        if self.retry_initial_ms == 0 || self.retry_max_ms < self.retry_initial_ms {
            return Err(ConfigError::Invalid(
                "thread_projection retry delays must be positive and max >= initial".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ThreadSemanticsConfig {
    pub enabled: bool,
    pub max_events: u32,
    pub max_total_chars: u32,
    pub max_event_chars: usize,
    pub max_batches_per_scan: u32,
    pub lease_secs: u64,
    pub scan_interval_ms: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
}

impl Default for ThreadSemanticsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_events: 50,
            max_total_chars: 50_000,
            max_event_chars: 10_000,
            max_batches_per_scan: 10,
            lease_secs: 60,
            scan_interval_ms: 1000,
            retry_initial_ms: 500,
            retry_max_ms: 10_000,
        }
    }
}

impl ThreadSemanticsConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.max_events == 0 || self.max_events > 500 {
            return Err(ConfigError::Invalid(
                "thread_semantics.max_events must be between 1 and 500".into(),
            ));
        }
        if self.max_total_chars == 0 || self.max_total_chars > 1_000_000 {
            return Err(ConfigError::Invalid(
                "thread_semantics.max_total_chars must be between 1 and 1000000".into(),
            ));
        }
        if self.max_event_chars == 0 || self.max_event_chars > 100_000 {
            return Err(ConfigError::Invalid(
                "thread_semantics.max_event_chars must be between 1 and 100000".into(),
            ));
        }
        if self.max_batches_per_scan == 0 || self.max_batches_per_scan > 100 {
            return Err(ConfigError::Invalid(
                "thread_semantics.max_batches_per_scan must be between 1 and 100".into(),
            ));
        }
        if self.lease_secs == 0 || self.lease_secs > 3600 || self.scan_interval_ms == 0 {
            return Err(ConfigError::Invalid(
                "thread_semantics lease and scan interval must be positive and bounded".into(),
            ));
        }
        if self.retry_initial_ms == 0 || self.retry_max_ms < self.retry_initial_ms {
            return Err(ConfigError::Invalid(
                "thread_semantics retry delays must be positive and max >= initial".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ThreadLinksConfig {
    pub enabled: bool,
    pub max_events: u32,
    pub max_total_chars: u32,
    pub max_batches_per_scan: u32,
    pub lease_secs: u64,
    pub scan_interval_ms: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
}

impl Default for ThreadLinksConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_events: 100,
            max_total_chars: 100_000,
            max_batches_per_scan: 10,
            lease_secs: 60,
            scan_interval_ms: 1500,
            retry_initial_ms: 500,
            retry_max_ms: 10_000,
        }
    }
}

impl ThreadLinksConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.max_events == 0 || self.max_events > 1000 {
            return Err(ConfigError::Invalid(
                "thread_links.max_events must be between 1 and 1000".into(),
            ));
        }
        if self.max_total_chars == 0 || self.max_total_chars > 2_000_000 {
            return Err(ConfigError::Invalid(
                "thread_links.max_total_chars must be between 1 and 2000000".into(),
            ));
        }
        if self.max_batches_per_scan == 0 || self.max_batches_per_scan > 100 {
            return Err(ConfigError::Invalid(
                "thread_links.max_batches_per_scan must be between 1 and 100".into(),
            ));
        }
        if self.lease_secs == 0 || self.lease_secs > 3600 || self.scan_interval_ms == 0 {
            return Err(ConfigError::Invalid(
                "thread_links lease and scan interval must be positive and bounded".into(),
            ));
        }
        if self.retry_initial_ms == 0 || self.retry_max_ms < self.retry_initial_ms {
            return Err(ConfigError::Invalid(
                "thread_links retry delays must be positive and max >= initial".into(),
            ));
        }
        Ok(())
    }
}

/// 结构化记忆维护与承诺提醒调度。只写持久化 Outbox，不直接发送消息。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FollowUpConfig {
    pub enabled: bool,
    pub scan_interval_ms: u64,
    pub horizon_secs: i64,
    pub batch_size: u32,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
}

impl Default for FollowUpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            scan_interval_ms: 30_000,
            horizon_secs: 604_800,
            batch_size: 200,
            retry_initial_ms: 1_000,
            retry_max_ms: 60_000,
        }
    }
}

impl FollowUpConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.scan_interval_ms < 1_000 || self.scan_interval_ms > 3_600_000 {
            return Err(ConfigError::Invalid(
                "follow_up.scan_interval_ms must be between 1000 and 3600000".into(),
            ));
        }
        if !(60..=31_536_000).contains(&self.horizon_secs) {
            return Err(ConfigError::Invalid(
                "follow_up.horizon_secs must be between 60 and 31536000".into(),
            ));
        }
        if !(1..=1000).contains(&self.batch_size) {
            return Err(ConfigError::Invalid(
                "follow_up.batch_size must be between 1 and 1000".into(),
            ));
        }
        if self.retry_initial_ms == 0 || self.retry_max_ms < self.retry_initial_ms {
            return Err(ConfigError::Invalid(
                "follow_up retry delays must be positive and max >= initial".into(),
            ));
        }
        Ok(())
    }
}
