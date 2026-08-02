//! 后台扫描与派生 Worker 的有界配置集合。
//!
//! 包含入站队列、历史回补、线程投影、线程语义、线程关联与跟进提醒。
//! 所有配置只在 QQBot 应用目录内生效，不读取数字人配置；预算业务不变量集中在领域层。

use std::path::PathBuf;

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

/// Owner Agenda 到期扫描。只将已到期的当前版本事项生成统一策略候选。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AgendaConfig {
    pub enabled: bool,
    pub scan_interval_ms: u64,
    pub batch_size: u32,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
}

impl Default for AgendaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            scan_interval_ms: 30_000,
            batch_size: 200,
            retry_initial_ms: 1_000,
            retry_max_ms: 60_000,
        }
    }
}

impl AgendaConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.scan_interval_ms < 1_000 || self.scan_interval_ms > 3_600_000 {
            return Err(ConfigError::Invalid(
                "agenda.scan_interval_ms must be between 1000 and 3600000".into(),
            ));
        }
        if !(1..=1000).contains(&self.batch_size) {
            return Err(ConfigError::Invalid(
                "agenda.batch_size must be between 1 and 1000".into(),
            ));
        }
        if self.retry_initial_ms == 0 || self.retry_max_ms < self.retry_initial_ms {
            return Err(ConfigError::Invalid(
                "agenda retry delays must be positive and max >= initial".into(),
            ));
        }
        Ok(())
    }
}

/// 结构化记忆维护与承诺提醒调度；只生成统一策略候选，不直接写 Outbox。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FollowUpConfig {
    pub enabled: bool,
    pub scan_interval_ms: u64,
    pub horizon_secs: i64,
    pub response_timeout_secs: i64,
    pub blocker_escalation_secs: i64,
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
            response_timeout_secs: 14_400,
            blocker_escalation_secs: 86_400,
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
        if !(300..=2_592_000).contains(&self.response_timeout_secs) {
            return Err(ConfigError::Invalid(
                "follow_up.response_timeout_secs must be between 300 and 2592000".into(),
            ));
        }
        if !(3_600..=31_536_000).contains(&self.blocker_escalation_secs) {
            return Err(ConfigError::Invalid(
                "follow_up.blocker_escalation_secs must be between 3600 and 31536000".into(),
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

/// 统一 Notification Policy 求值 Worker 配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NotificationPolicyConfig {
    pub enabled: bool,
    pub worker_id: String,
    pub batch_size: u32,
    pub lease_secs: u64,
    pub scan_interval_ms: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
    pub recovery_limit: u32,
    /// 启动时历史直写 Outbox 协调的全局租约持有者标识。
    pub reconciliation_worker_id: String,
    pub reconciliation_lease_secs: u64,
    pub reconciliation_page_size: u32,
    pub reconciliation_max_rows: u32,
    pub reconciliation_deadline_secs: u64,
}

impl Default for NotificationPolicyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            worker_id: "qqbot-notification-policy-v1".into(),
            batch_size: 100,
            lease_secs: 60,
            scan_interval_ms: 1_000,
            retry_initial_ms: 1_000,
            retry_max_ms: 60_000,
            recovery_limit: 1_000,
            reconciliation_worker_id: "qqbot-legacy-owner-outbox-v1".into(),
            reconciliation_lease_secs: 60,
            reconciliation_page_size: 100,
            reconciliation_max_rows: 10_000,
            reconciliation_deadline_secs: 120,
        }
    }
}

impl NotificationPolicyConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.worker_id.trim().is_empty() || self.worker_id.len() > 128 {
            return Err(ConfigError::Invalid(
                "notification_policy.worker_id must be non-empty and at most 128 bytes".into(),
            ));
        }
        if !(1..=1000).contains(&self.batch_size) {
            return Err(ConfigError::Invalid(
                "notification_policy.batch_size must be between 1 and 1000".into(),
            ));
        }
        if !(1..=3600).contains(&self.lease_secs) {
            return Err(ConfigError::Invalid(
                "notification_policy.lease_secs must be between 1 and 3600".into(),
            ));
        }
        if self.scan_interval_ms < 100 || self.scan_interval_ms > 3_600_000 {
            return Err(ConfigError::Invalid(
                "notification_policy.scan_interval_ms must be between 100 and 3600000".into(),
            ));
        }
        if self.retry_initial_ms == 0 || self.retry_max_ms < self.retry_initial_ms {
            return Err(ConfigError::Invalid(
                "notification_policy retry delays must be positive and max >= initial".into(),
            ));
        }
        if !(1..=1000).contains(&self.recovery_limit) {
            return Err(ConfigError::Invalid(
                "notification_policy.recovery_limit must be between 1 and 1000".into(),
            ));
        }
        if self.reconciliation_worker_id.trim().is_empty()
            || self.reconciliation_worker_id.len() > 128
        {
            return Err(ConfigError::Invalid(
                "notification_policy.reconciliation_worker_id must be non-empty and at most 128 bytes"
                    .into(),
            ));
        }
        if !(1..=3600).contains(&self.reconciliation_lease_secs)
            || !(1..=1000).contains(&self.reconciliation_page_size)
            || !(1..=100_000).contains(&self.reconciliation_max_rows)
            || !(1..=300).contains(&self.reconciliation_deadline_secs)
        {
            return Err(ConfigError::Invalid(
                "notification_policy reconciliation bounds are invalid".into(),
            ));
        }
        Ok(())
    }
}

/// B4 账号会话目录同步配置。
///
/// 目录同步具备 single-flight、TTL、批次上限、整体 deadline、指数退避、shutdown。
/// 不在每次 WebSocket 重连时无条件下载完整目录（TTL 内跳过）。
/// 1 MiB 上限拒绝时保持 uncertain，不提高上限、不转空数组。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DirectorySyncConfig {
    pub enabled: bool,
    /// 快照 TTL（秒）。TTL 内跳过完整下载。
    pub snapshot_ttl_secs: u64,
    /// 单次同步的整体 deadline（秒）。
    pub sync_deadline_secs: u64,
    /// 单次同步的条目上限。
    pub max_entries: u32,
    /// 扫描间隔（毫秒）。目录同步是周期性后台任务。
    pub scan_interval_ms: u64,
    /// 错误退避初始延迟（毫秒）。
    pub retry_initial_ms: u64,
    /// 错误退避最大延迟（毫秒）。
    pub retry_max_ms: u64,
}

impl Default for DirectorySyncConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            snapshot_ttl_secs: 3600,
            sync_deadline_secs: 30,
            max_entries: 5000,
            scan_interval_ms: 300_000,
            retry_initial_ms: 1_000,
            retry_max_ms: 60_000,
        }
    }
}

impl DirectorySyncConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.snapshot_ttl_secs == 0 {
            return Err(ConfigError::Invalid(
                "directory_sync.snapshot_ttl_secs must be positive".into(),
            ));
        }
        if self.sync_deadline_secs == 0 || self.sync_deadline_secs > 120 {
            return Err(ConfigError::Invalid(
                "directory_sync.sync_deadline_secs must be between 1 and 120".into(),
            ));
        }
        if self.max_entries == 0 || self.max_entries > 100_000 {
            return Err(ConfigError::Invalid(
                "directory_sync.max_entries must be between 1 and 100000".into(),
            ));
        }
        if self.scan_interval_ms < 60_000 || self.scan_interval_ms > 86_400_000 {
            return Err(ConfigError::Invalid(
                "directory_sync.scan_interval_ms must be between 60000 and 86400000".into(),
            ));
        }
        if self.retry_initial_ms == 0 || self.retry_max_ms < self.retry_initial_ms {
            return Err(ConfigError::Invalid(
                "directory_sync retry delays must be positive and max >= initial".into(),
            ));
        }
        Ok(())
    }

    /// 构造领域层有界预算。
    pub fn budget(&self) -> personal_secretary::DirectorySyncBudget {
        personal_secretary::DirectorySyncBudget {
            snapshot_ttl_secs: self.snapshot_ttl_secs,
            sync_deadline_secs: self.sync_deadline_secs,
            max_entries: self.max_entries,
            retry_initial_ms: self.retry_initial_ms,
            retry_max_ms: self.retry_max_ms,
        }
    }
}

/// B3 本地撤回 WAL 配置。WAL 成功落盘后回调即可返回；后台异步转存 MySQL。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RecallWalConfig {
    pub path: PathBuf,
    pub max_bytes: u64,
    pub drain_interval_ms: u64,
    pub key_env: String,
    pub quarantine_dir: PathBuf,
}

impl Default for RecallWalConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("data/qqbot-recall.wal"),
            max_bytes: 16 * 1024 * 1024,
            drain_interval_ms: 1_000,
            key_env: "QQBOT_RECALL_WAL_KEY".into(),
            quarantine_dir: PathBuf::from("data/qqbot-recall-quarantine"),
        }
    }
}

impl RecallWalConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.path.as_os_str().is_empty() {
            return Err(ConfigError::Invalid(
                "recall_wal.path must not be empty".into(),
            ));
        }
        if self.key_env.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "recall_wal.key_env must not be empty".into(),
            ));
        }
        if self.quarantine_dir.as_os_str().is_empty() {
            return Err(ConfigError::Invalid(
                "recall_wal.quarantine_dir must not be empty".into(),
            ));
        }
        if !(1024..=1_073_741_824).contains(&self.max_bytes) {
            return Err(ConfigError::Invalid(
                "recall_wal.max_bytes must be between 1024 and 1073741824".into(),
            ));
        }
        if !(10..=60_000).contains(&self.drain_interval_ms) {
            return Err(ConfigError::Invalid(
                "recall_wal.drain_interval_ms must be between 10 and 60000".into(),
            ));
        }
        Ok(())
    }
}

/// B6 富消息 Artifact 配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ArtifactConfig {
    pub enabled: bool,
    /// 默认 TTL（秒）。None/0 表示不设默认 TTL（仅显式过期策略）。
    pub default_ttl_secs: u64,
    /// TTL 扫描间隔（毫秒）。
    pub ttl_scan_interval_ms: u64,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_ttl_secs: 86_400,
            ttl_scan_interval_ms: 60_000,
        }
    }
}

impl ArtifactConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.ttl_scan_interval_ms < 1_000 || self.ttl_scan_interval_ms > 3_600_000 {
            return Err(ConfigError::Invalid(
                "artifact.ttl_scan_interval_ms must be between 1000 and 3600000".into(),
            ));
        }
        if self.default_ttl_secs > 31_536_000 {
            return Err(ConfigError::Invalid(
                "artifact.default_ttl_secs must be <= 31536000".into(),
            ));
        }
        Ok(())
    }
}

/// 结构化记忆候选提取 Worker 配置。
///
/// 独立可取消可退避的持久游标扫描：从 `secretary_source_events` 提取
/// person/project/commitment 候选，Owner 批准后才落为 MemoryFact。
/// 默认关闭（保守），需要显式开启；`batch_size` 是单次扫描最多处理的批次数。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MemoryCandidatesConfig {
    pub enabled: bool,
    /// 扫描间隔（毫秒）。
    pub scan_interval_ms: u64,
    /// 单次扫描最多处理的批次数（每批事件数由 max_events_per_batch 限定）。
    pub batch_size: u32,
    /// 批次租约时长（秒）。
    pub lease_secs: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
    /// 每批最多事件数（1..=100）。
    pub max_events_per_batch: u32,
    /// 单条事件最多字符数（1..=4000）。
    pub max_event_chars: u32,
    /// 整批输入总字符上限（1..=16000；本切片硬上限，环境变量不可突破，
    /// 防止成本边界被配置意外放大）。
    pub max_total_input_chars: u32,
    /// 提取器版本标识，参与候选确定性指纹（非空且 ≤32 字节）。
    pub extractor_version: String,
}

impl Default for MemoryCandidatesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scan_interval_ms: 30_000,
            batch_size: 10,
            lease_secs: 60,
            retry_initial_ms: 500,
            retry_max_ms: 10_000,
            max_events_per_batch: 20,
            max_event_chars: 2_000,
            max_total_input_chars: 16_000,
            extractor_version: "v1".into(),
        }
    }
}

impl MemoryCandidatesConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.scan_interval_ms < 1_000 || self.scan_interval_ms > 3_600_000 {
            return Err(ConfigError::Invalid(
                "memory_candidates.scan_interval_ms must be between 1000 and 3600000".into(),
            ));
        }
        if !(1..=100).contains(&self.batch_size) {
            return Err(ConfigError::Invalid(
                "memory_candidates.batch_size must be between 1 and 100".into(),
            ));
        }
        if !(1..=3600).contains(&self.lease_secs) {
            return Err(ConfigError::Invalid(
                "memory_candidates.lease_secs must be between 1 and 3600".into(),
            ));
        }
        if self.retry_initial_ms == 0 || self.retry_max_ms < self.retry_initial_ms {
            return Err(ConfigError::Invalid(
                "memory_candidates retry delays must be positive and max >= initial".into(),
            ));
        }
        if !(1..=100).contains(&self.max_events_per_batch) {
            return Err(ConfigError::Invalid(
                "memory_candidates.max_events_per_batch must be between 1 and 100".into(),
            ));
        }
        if !(1..=4_000).contains(&self.max_event_chars) {
            return Err(ConfigError::Invalid(
                "memory_candidates.max_event_chars must be between 1 and 4000".into(),
            ));
        }
        // 总输入硬上限 16000：即使环境变量覆盖也不得放大成本边界。
        if !(1..=16_000).contains(&self.max_total_input_chars) {
            return Err(ConfigError::Invalid(
                "memory_candidates.max_total_input_chars must be between 1 and 16000".into(),
            ));
        }
        if self.extractor_version.trim().is_empty() || self.extractor_version.len() > 32 {
            return Err(ConfigError::Invalid(
                "memory_candidates.extractor_version must be non-empty and at most 32 bytes".into(),
            ));
        }
        Ok(())
    }
}

/// B7 健康快照配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HealthConfig {
    pub enabled: bool,
    /// 聚合缓存 TTL（秒）。
    pub cache_ttl_secs: u64,
    /// 周期结构化日志间隔（毫秒）。
    pub log_interval_ms: u64,
    /// 最近一次 Worker 成功超过该阈值后降级（秒）。
    pub worker_success_stale_secs: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cache_ttl_secs: 5,
            log_interval_ms: 30_000,
            worker_success_stale_secs: 300,
        }
    }
}

impl HealthConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.cache_ttl_secs == 0 || self.cache_ttl_secs > 300 {
            return Err(ConfigError::Invalid(
                "health.cache_ttl_secs must be between 1 and 300".into(),
            ));
        }
        if !(1..=86_400).contains(&self.worker_success_stale_secs) {
            return Err(ConfigError::Invalid(
                "health.worker_success_stale_secs must be between 1 and 86400".into(),
            ));
        }
        Ok(())
    }
}
