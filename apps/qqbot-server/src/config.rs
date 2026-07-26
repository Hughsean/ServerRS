use std::path::PathBuf;

use personal_secretary::BackfillBudget;
use qq_open_platform::QqBotCredentials;
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub napcat: NapCatConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub ingestion: IngestionConfig,
    #[serde(default)]
    pub backfill: BackfillConfig,
    #[serde(default)]
    pub thread_projection: ThreadProjectionConfig,
    #[serde(default)]
    pub thread_semantics: ThreadSemanticsConfig,
    #[serde(default)]
    pub thread_links: ThreadLinksConfig,
    #[serde(default)]
    pub follow_up: FollowUpConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub qq_open_platform: QqOpenPlatformConfig,
    #[serde(default)]
    pub whitelist: WhitelistConfig,
    #[serde(default)]
    pub action_planner: ActionPlannerConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NapCatConfig {
    pub ws_url: String,
    pub http_base_url: String,
    pub self_qq_id: i64,
    #[serde(default = "default_reconnect_initial_secs")]
    pub reconnect_initial_secs: u64,
    #[serde(default = "default_reconnect_max_secs")]
    pub reconnect_max_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default = "default_database_max_connections")]
    pub max_connections: u32,
}

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
    fn validate(&self) -> Result<(), ConfigError> {
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
    fn validate(&self) -> Result<(), ConfigError> {
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
pub struct ActionPlannerConfig {
    pub enabled: bool,
    pub max_batches_per_scan: u32,
    pub lease_secs: u64,
    pub scan_interval_ms: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
}

impl Default for ActionPlannerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_batches_per_scan: 10,
            lease_secs: 60,
            scan_interval_ms: 2000,
            retry_initial_ms: 500,
            retry_max_ms: 10_000,
        }
    }
}

impl ActionPlannerConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.max_batches_per_scan == 0 || self.max_batches_per_scan > 100 {
            return Err(ConfigError::Invalid(
                "action_planner.max_batches_per_scan must be between 1 and 100".into(),
            ));
        }
        if self.lease_secs == 0 || self.lease_secs > 3600 || self.scan_interval_ms == 0 {
            return Err(ConfigError::Invalid(
                "action_planner lease and scan interval must be positive and bounded".into(),
            ));
        }
        if self.retry_initial_ms == 0 || self.retry_max_ms < self.retry_initial_ms {
            return Err(ConfigError::Invalid(
                "action_planner retry delays must be positive and max >= initial".into(),
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
    fn validate(&self) -> Result<(), ConfigError> {
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
    fn validate(&self) -> Result<(), ConfigError> {
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

/// QQBot 独立 LLM 配置。API Key 只允许来自进程环境或本地文件，禁止写入 TOML。
/// 当前垂直切片仅用于有界线程语义提取，不允许模型直接访问数据库、网络工具或消息发送。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LlmConfig {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub api_key_file: Option<PathBuf>,
    pub connect_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub max_input_chars: usize,
    pub max_output_tokens: u32,
    pub max_response_bytes: usize,
    pub temperature: f64,
    pub max_candidates_per_kind: usize,
    pub reasoning_mode: LlmReasoningMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmReasoningMode {
    #[default]
    ProviderDefault,
    QwenNoThink,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: String::new(),
            api_key_file: None,
            connect_timeout_secs: 10,
            request_timeout_secs: 60,
            max_input_chars: 60_000,
            max_output_tokens: 2_000,
            max_response_bytes: 1_048_576,
            temperature: 0.1,
            max_candidates_per_kind: 20,
            reasoning_mode: LlmReasoningMode::ProviderDefault,
        }
    }
}

impl LlmConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if !self.enabled {
            return Ok(());
        }
        if self.model.trim().is_empty() || self.model.len() > 191 {
            return Err(ConfigError::Invalid(
                "llm.model must contain 1..=191 bytes when enabled".into(),
            ));
        }
        let url = url::Url::parse(&self.base_url).map_err(|error| {
            ConfigError::Invalid(format!("llm.base_url must be an absolute URL: {error}"))
        })?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ConfigError::Invalid(
                "llm.base_url must not contain credentials, query, or fragment".into(),
            ));
        }
        match url.scheme() {
            "https" => {}
            "http" if url.host_str().is_some_and(is_loopback_host) => {}
            _ => {
                return Err(ConfigError::Invalid(
                    "llm.base_url must use HTTPS; plain HTTP is allowed only on loopback".into(),
                ));
            }
        }
        if !(1..=300).contains(&self.connect_timeout_secs)
            || !(1..=600).contains(&self.request_timeout_secs)
        {
            return Err(ConfigError::Invalid(
                "llm timeouts must be positive and bounded".into(),
            ));
        }
        if !(1_000..=1_000_000).contains(&self.max_input_chars) {
            return Err(ConfigError::Invalid(
                "llm.max_input_chars must be in 1000..=1000000".into(),
            ));
        }
        if !(1..=32_768).contains(&self.max_output_tokens) {
            return Err(ConfigError::Invalid(
                "llm.max_output_tokens must be in 1..=32768".into(),
            ));
        }
        if !(1_024..=10_485_760).contains(&self.max_response_bytes) {
            return Err(ConfigError::Invalid(
                "llm.max_response_bytes must be in 1024..=10485760".into(),
            ));
        }
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err(ConfigError::Invalid(
                "llm.temperature must be finite and in 0..=2".into(),
            ));
        }
        if !(1..=100).contains(&self.max_candidates_per_kind) {
            return Err(ConfigError::Invalid(
                "llm.max_candidates_per_kind must be in 1..=100".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn api_key(&self) -> Result<Option<String>, ConfigError> {
        if let Ok(value) = std::env::var("QQBOT_LLM_API_KEY")
            && !value.trim().is_empty()
        {
            return Ok(Some(value));
        }
        let Some(path) = &self.api_key_file else {
            return Ok(None);
        };
        let value = std::fs::read_to_string(path).map_err(|error| {
            ConfigError::Invalid(format!("failed to read llm.api_key_file: {error}"))
        })?;
        let value = value.trim().to_owned();
        if value.is_empty() {
            return Err(ConfigError::Invalid("llm.api_key_file is empty".into()));
        }
        Ok(Some(value))
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// 官方 QQ Bot 通道。Secret 只允许来自进程环境或本地文件，不接受 TOML 明文字段。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct QqOpenPlatformConfig {
    pub enabled: bool,
    pub app_id: String,
    pub client_secret_file: Option<PathBuf>,
    pub owner_openid: String,
    pub reconnect_initial_ms: u64,
    pub reconnect_max_ms: u64,
    pub notification_lease_secs: u64,
}

impl Default for QqOpenPlatformConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            app_id: String::new(),
            client_secret_file: None,
            owner_openid: String::new(),
            reconnect_initial_ms: 1_000,
            reconnect_max_ms: 60_000,
            notification_lease_secs: 60,
        }
    }
}

impl QqOpenPlatformConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if !self.enabled {
            return Ok(());
        }
        if self.app_id.trim().is_empty() || self.app_id.len() > 191 {
            return Err(ConfigError::Invalid(
                "qq_open_platform.app_id must contain 1..=191 bytes when enabled".into(),
            ));
        }
        if self.owner_openid.trim().is_empty() || self.owner_openid.len() > 191 {
            return Err(ConfigError::Invalid(
                "qq_open_platform.owner_openid must contain 1..=191 bytes when enabled".into(),
            ));
        }
        if self.reconnect_initial_ms == 0 || self.reconnect_max_ms < self.reconnect_initial_ms {
            return Err(ConfigError::Invalid(
                "qq_open_platform reconnect delays must be positive and max >= initial".into(),
            ));
        }
        if !(1..=3600).contains(&self.notification_lease_secs) {
            return Err(ConfigError::Invalid(
                "qq_open_platform.notification_lease_secs must be in 1..=3600".into(),
            ));
        }
        if std::env::var("QQBOT_OPEN_PLATFORM_CLIENT_SECRET")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .is_none()
            && self.client_secret_file.is_none()
        {
            return Err(ConfigError::Invalid(
                "enabled QQ Open Platform requires QQBOT_OPEN_PLATFORM_CLIENT_SECRET or client_secret_file"
                    .into(),
            ));
        }
        Ok(())
    }

    pub fn credentials(&self) -> Result<QqBotCredentials, ConfigError> {
        self.validate()?;
        let secret = if let Ok(value) = std::env::var("QQBOT_OPEN_PLATFORM_CLIENT_SECRET")
            && !value.trim().is_empty()
        {
            value
        } else if let Some(path) = &self.client_secret_file {
            std::fs::read_to_string(path)
                .map_err(|error| {
                    ConfigError::Invalid(format!(
                        "failed to read QQ Open Platform client_secret_file: {error}"
                    ))
                })?
                .trim()
                .to_owned()
        } else {
            return Err(ConfigError::Invalid(
                "QQ Open Platform client secret is unavailable".into(),
            ));
        };
        QqBotCredentials::new(self.app_id.clone(), secret)
            .map_err(|error| ConfigError::Invalid(error.to_string()))
    }
}

/// 群白名单配置。只有白名单内的群消息才会被处理和持久化。
///
/// 白名单文件是 JSON 格式：`{"groups": [671260344, ...]}`。
/// `whitelist_file` 为相对路径时以配置文件目录为基准；不配时表示不启用白名单
/// （所有群消息都会被处理）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WhitelistConfig {
    /// 白名单 JSON 文件路径。不配则不启用白名单过滤。
    pub whitelist_file: Option<PathBuf>,
}

impl WhitelistConfig {
    /// 解析白名单文件路径：相对路径以 `config_dir` 为基准，绝对路径直接使用。
    fn resolve_path(&self, config_dir: &std::path::Path) -> Option<PathBuf> {
        self.whitelist_file.as_ref().map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                config_dir.join(path)
            }
        })
    }

    fn validate(&self, config_dir: &std::path::Path) -> Result<(), ConfigError> {
        if let Some(path) = self.resolve_path(config_dir) {
            if !path.exists() {
                return Err(ConfigError::Invalid(format!(
                    "whitelist.whitelist_file 指向的文件不存在: {}",
                    path.display()
                )));
            }
            // 尝试解析 JSON，确认格式正确。
            let content = std::fs::read_to_string(&path).map_err(|error| {
                ConfigError::Invalid(format!("读取白名单文件失败 {}: {error}", path.display()))
            })?;
            let parsed: WhitelistFile = serde_json::from_str(&content).map_err(|error| {
                ConfigError::Invalid(format!(
                    "白名单文件 JSON 格式错误 {}: {error}",
                    path.display()
                ))
            })?;
            if parsed.groups.is_empty() {
                return Err(ConfigError::Invalid(
                    "白名单文件 groups 不能为空数组；如需放行所有群，请删除 whitelist_file 配置"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    /// 加载白名单群号集合。`whitelist_file` 为 None 时返回空集合（表示不启用过滤）。
    /// 相对路径以 `config_dir` 为基准。
    ///
    /// 拒绝空数组（防止文件被改为空时 fail-open）和非正群号。
    pub fn load_groups(
        &self,
        config_dir: &std::path::Path,
    ) -> Result<std::collections::HashSet<i64>, ConfigError> {
        let Some(path) = self.resolve_path(config_dir) else {
            return Ok(std::collections::HashSet::new());
        };
        let content = std::fs::read_to_string(&path).map_err(|error| {
            ConfigError::Invalid(format!("读取白名单文件失败 {}: {error}", path.display()))
        })?;
        let parsed: WhitelistFile = serde_json::from_str(&content).map_err(|error| {
            ConfigError::Invalid(format!(
                "白名单文件 JSON 格式错误 {}: {error}",
                path.display()
            ))
        })?;
        if parsed.groups.is_empty() {
            return Err(ConfigError::Invalid(
                "白名单文件 groups 不能为空数组；如需放行所有群，请删除 whitelist_file 配置".into(),
            ));
        }
        for group_id in &parsed.groups {
            if *group_id <= 0 {
                return Err(ConfigError::Invalid(format!(
                    "白名单文件包含非法群号 {group_id}；群号必须为正整数"
                )));
            }
        }
        Ok(parsed.groups.into_iter().collect())
    }
}

/// 白名单 JSON 文件结构。
#[derive(Debug, Deserialize)]
struct WhitelistFile {
    groups: Vec<i64>,
}

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

/// 环境变量覆盖只保留四种常见、类型明确的机械操作。
/// 特殊枚举和浮点语义仍在调用处显式解析，避免形成难以调试的万能配置 DSL。
macro_rules! apply_env_field {
    ($config:expr, $field:ident, $name:literal, bool) => {
        if let Ok(value) = std::env::var($name) {
            ($config).$field = parse_bool($name, &value)?;
        }
    };
    ($config:expr, $field:ident, $name:literal, positive) => {
        if let Ok(value) = std::env::var($name) {
            ($config).$field = parse_positive($name, &value)?;
        }
    };
    ($config:expr, $field:ident, $name:literal, non_empty) => {
        if let Ok(value) = std::env::var($name)
            && !value.trim().is_empty()
        {
            ($config).$field = value;
        }
    };
    ($config:expr, $field:ident, $name:literal, path) => {
        if let Ok(value) = std::env::var($name)
            && !value.trim().is_empty()
        {
            ($config).$field = Some(PathBuf::from(value));
        }
    };
}

macro_rules! apply_env_fields {
    ($config:expr; $( $kind:ident { $( $field:ident => $name:literal ),* $(,)? } ),* $(,)?) => {
        $(
            $(apply_env_field!($config, $field, $name, $kind);)*
        )*
    };
}

impl AppConfig {
    /// QQBot 使用应用目录内的独立配置入口，不读取数字人的根 `.env` 或 `CONFIG_PATH`。
    ///
    /// 返回 `(config, config_dir)`，`config_dir` 是配置文件所在目录，
    /// 用于解析白名单等相对路径。
    pub fn load() -> Result<(Self, PathBuf), ConfigError> {
        let config_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config");
        let _ = dotenvy::from_path(config_dir.join(".env"));
        let path = std::env::var_os("QQBOT_CONFIG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| config_dir.join("qqbot.toml"));
        let content = std::fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.clone(),
            source,
        })?;
        let mut config: Self = toml::from_str(&content).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;
        config.apply_env_overrides()?;
        let config_dir = path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        config.validate(&config_dir)?;
        Ok((config, config_dir))
    }

    fn apply_env_overrides(&mut self) -> Result<(), ConfigError> {
        apply_env_fields!(&mut self.napcat;
            non_empty {
                ws_url => "NAPCAT_WS_URL",
                http_base_url => "NAPCAT_HTTP_BASE_URL",
            },
            positive { self_qq_id => "NAPCAT_SELF_QQ_ID" },
        );
        apply_env_fields!(&mut self.database;
            non_empty { url => "QQBOT_DATABASE_URL" },
            positive { max_connections => "QQBOT_DATABASE_MAX_CONNECTIONS" },
        );
        apply_env_fields!(&mut self.ingestion;
            positive {
                queue_capacity => "QQBOT_INGESTION_QUEUE_CAPACITY",
                retry_initial_ms => "QQBOT_INGESTION_RETRY_INITIAL_MS",
                retry_max_ms => "QQBOT_INGESTION_RETRY_MAX_MS",
                shutdown_drain_timeout_secs => "QQBOT_INGESTION_SHUTDOWN_DRAIN_TIMEOUT_SECS",
            },
        );
        apply_backfill_env(&mut self.backfill)?;
        apply_thread_projection_env(&mut self.thread_projection)?;
        apply_thread_semantics_env(&mut self.thread_semantics)?;
        apply_thread_links_env(&mut self.thread_links)?;
        apply_follow_up_env(&mut self.follow_up)?;
        apply_llm_env(&mut self.llm)?;
        apply_qq_open_platform_env(&mut self.qq_open_platform)?;
        apply_whitelist_env(&mut self.whitelist)?;
        Ok(())
    }

    fn validate(&self, config_dir: &std::path::Path) -> Result<(), ConfigError> {
        validate_loopback_url(&self.napcat.ws_url, &["ws", "wss"], "napcat.ws_url")?;
        validate_loopback_url(
            &self.napcat.http_base_url,
            &["http", "https"],
            "napcat.http_base_url",
        )?;
        if self.napcat.self_qq_id <= 0 {
            return Err(ConfigError::Invalid(
                "napcat.self_qq_id must be a positive QQ number".into(),
            ));
        }
        if self.napcat.reconnect_initial_secs == 0
            || self.napcat.reconnect_max_secs < self.napcat.reconnect_initial_secs
        {
            return Err(ConfigError::Invalid(
                "NapCat reconnect delays must be positive and max >= initial".into(),
            ));
        }
        validate_url(&self.database.url, &["mysql"], "database.url")?;
        if self.database.max_connections == 0 {
            return Err(ConfigError::Invalid(
                "database.max_connections must be positive".into(),
            ));
        }
        if self.ingestion.queue_capacity == 0 || self.ingestion.queue_capacity > 65_536 {
            return Err(ConfigError::Invalid(
                "ingestion.queue_capacity must be between 1 and 65536".into(),
            ));
        }
        if self.ingestion.retry_initial_ms == 0
            || self.ingestion.retry_max_ms < self.ingestion.retry_initial_ms
        {
            return Err(ConfigError::Invalid(
                "ingestion retry delays must be positive and max >= initial".into(),
            ));
        }
        if self.ingestion.shutdown_drain_timeout_secs == 0 {
            return Err(ConfigError::Invalid(
                "ingestion.shutdown_drain_timeout_secs must be positive".into(),
            ));
        }
        // 回补预算业务不变量在领域层集中定义，配置层调用其校验。
        self.backfill.budget()?;
        self.thread_projection.validate()?;
        self.thread_semantics.validate()?;
        self.thread_links.validate()?;
        self.follow_up.validate()?;
        self.llm.validate()?;
        self.qq_open_platform.validate()?;
        self.whitelist.validate(config_dir)?;
        self.action_planner.validate()?;
        Ok(())
    }
}

fn validate_url(value: &str, schemes: &[&str], field: &str) -> Result<(), ConfigError> {
    let url = url::Url::parse(value).map_err(|error| {
        ConfigError::Invalid(format!("{field} must be an absolute URL: {error}"))
    })?;
    if !schemes.contains(&url.scheme()) {
        return Err(ConfigError::Invalid(format!(
            "{field} must use one of these schemes: {}",
            schemes.join(", ")
        )));
    }
    Ok(())
}

fn validate_loopback_url(value: &str, schemes: &[&str], field: &str) -> Result<(), ConfigError> {
    validate_url(value, schemes, field)?;
    let url = url::Url::parse(value).map_err(|error| {
        ConfigError::Invalid(format!("{field} must be an absolute URL: {error}"))
    })?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::Invalid(format!(
            "{field} must not contain credentials, query, or fragment"
        )));
    }
    if !url.host_str().is_some_and(is_loopback_host) {
        return Err(ConfigError::Invalid(format!(
            "{field} must use a loopback host because NapCat authentication is disabled"
        )));
    }
    Ok(())
}

fn default_reconnect_initial_secs() -> u64 {
    1
}

fn default_reconnect_max_secs() -> u64 {
    60
}

fn default_database_max_connections() -> u32 {
    5
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

fn parse_positive<T>(name: &str, value: &str) -> Result<T, ConfigError>
where
    T: std::str::FromStr + PartialOrd + Default,
{
    let parsed = value
        .parse::<T>()
        .map_err(|_| ConfigError::Invalid(format!("{name} must be a positive integer")))?;
    if parsed <= T::default() {
        return Err(ConfigError::Invalid(format!(
            "{name} must be a positive integer"
        )));
    }
    Ok(parsed)
}

/// 应用 `QQBOT_BACKFILL_*` 环境变量覆盖。只使用 QQBot 专属前缀，不读取数字人配置。
fn apply_backfill_env(config: &mut BackfillConfig) -> Result<(), ConfigError> {
    apply_env_fields!(config;
        bool { enabled => "QQBOT_BACKFILL_ENABLED" },
        positive {
            page_size => "QQBOT_BACKFILL_PAGE_SIZE",
            max_pages_per_scope => "QQBOT_BACKFILL_MAX_PAGES_PER_SCOPE",
            max_events_per_run => "QQBOT_BACKFILL_MAX_EVENTS_PER_RUN",
            max_concurrency => "QQBOT_BACKFILL_MAX_CONCURRENCY",
            lease_secs => "QQBOT_BACKFILL_LEASE_SECS",
            retry_initial_ms => "QQBOT_BACKFILL_RETRY_INITIAL_MS",
            retry_max_ms => "QQBOT_BACKFILL_RETRY_MAX_MS",
        },
    );
    Ok(())
}

fn apply_thread_projection_env(config: &mut ThreadProjectionConfig) -> Result<(), ConfigError> {
    apply_env_fields!(config;
        bool { enabled => "QQBOT_THREAD_PROJECTION_ENABLED" },
        positive {
            batch_size => "QQBOT_THREAD_PROJECTION_BATCH_SIZE",
            max_batches_per_scan => "QQBOT_THREAD_PROJECTION_MAX_BATCHES_PER_SCAN",
            same_conversation_window_secs => "QQBOT_THREAD_PROJECTION_WINDOW_SECS",
            lease_secs => "QQBOT_THREAD_PROJECTION_LEASE_SECS",
            scan_interval_ms => "QQBOT_THREAD_PROJECTION_SCAN_INTERVAL_MS",
            retry_initial_ms => "QQBOT_THREAD_PROJECTION_RETRY_INITIAL_MS",
            retry_max_ms => "QQBOT_THREAD_PROJECTION_RETRY_MAX_MS",
        },
    );
    Ok(())
}

fn apply_thread_semantics_env(config: &mut ThreadSemanticsConfig) -> Result<(), ConfigError> {
    apply_env_fields!(config;
        bool { enabled => "QQBOT_THREAD_SEMANTICS_ENABLED" },
        positive {
            max_events => "QQBOT_THREAD_SEMANTICS_MAX_EVENTS",
            max_total_chars => "QQBOT_THREAD_SEMANTICS_MAX_TOTAL_CHARS",
            max_event_chars => "QQBOT_THREAD_SEMANTICS_MAX_EVENT_CHARS",
            max_batches_per_scan => "QQBOT_THREAD_SEMANTICS_MAX_BATCHES_PER_SCAN",
            lease_secs => "QQBOT_THREAD_SEMANTICS_LEASE_SECS",
            scan_interval_ms => "QQBOT_THREAD_SEMANTICS_SCAN_INTERVAL_MS",
            retry_initial_ms => "QQBOT_THREAD_SEMANTICS_RETRY_INITIAL_MS",
            retry_max_ms => "QQBOT_THREAD_SEMANTICS_RETRY_MAX_MS",
        },
    );
    Ok(())
}

fn apply_thread_links_env(config: &mut ThreadLinksConfig) -> Result<(), ConfigError> {
    apply_env_fields!(config;
        bool { enabled => "QQBOT_THREAD_LINKS_ENABLED" },
        positive {
            max_events => "QQBOT_THREAD_LINKS_MAX_EVENTS",
            max_total_chars => "QQBOT_THREAD_LINKS_MAX_TOTAL_CHARS",
            max_batches_per_scan => "QQBOT_THREAD_LINKS_MAX_BATCHES_PER_SCAN",
            lease_secs => "QQBOT_THREAD_LINKS_LEASE_SECS",
            scan_interval_ms => "QQBOT_THREAD_LINKS_SCAN_INTERVAL_MS",
            retry_initial_ms => "QQBOT_THREAD_LINKS_RETRY_INITIAL_MS",
            retry_max_ms => "QQBOT_THREAD_LINKS_RETRY_MAX_MS",
        },
    );
    Ok(())
}

fn apply_follow_up_env(config: &mut FollowUpConfig) -> Result<(), ConfigError> {
    apply_env_fields!(config;
        bool { enabled => "QQBOT_FOLLOW_UP_ENABLED" },
        positive {
            scan_interval_ms => "QQBOT_FOLLOW_UP_SCAN_INTERVAL_MS",
            horizon_secs => "QQBOT_FOLLOW_UP_HORIZON_SECS",
            batch_size => "QQBOT_FOLLOW_UP_BATCH_SIZE",
            retry_initial_ms => "QQBOT_FOLLOW_UP_RETRY_INITIAL_MS",
            retry_max_ms => "QQBOT_FOLLOW_UP_RETRY_MAX_MS",
        },
    );
    Ok(())
}

fn apply_llm_env(config: &mut LlmConfig) -> Result<(), ConfigError> {
    apply_env_fields!(config;
        bool { enabled => "QQBOT_LLM_ENABLED" },
        non_empty {
            base_url => "QQBOT_LLM_BASE_URL",
            model => "QQBOT_LLM_MODEL",
        },
        path { api_key_file => "QQBOT_LLM_API_KEY_FILE" },
        positive {
            connect_timeout_secs => "QQBOT_LLM_CONNECT_TIMEOUT_SECS",
            request_timeout_secs => "QQBOT_LLM_REQUEST_TIMEOUT_SECS",
            max_input_chars => "QQBOT_LLM_MAX_INPUT_CHARS",
            max_output_tokens => "QQBOT_LLM_MAX_OUTPUT_TOKENS",
            max_response_bytes => "QQBOT_LLM_MAX_RESPONSE_BYTES",
            max_candidates_per_kind => "QQBOT_LLM_MAX_CANDIDATES_PER_KIND",
        },
    );
    if let Ok(value) = std::env::var("QQBOT_LLM_TEMPERATURE") {
        config.temperature = value
            .parse()
            .map_err(|_| ConfigError::Invalid("QQBOT_LLM_TEMPERATURE must be a number".into()))?;
    }
    if let Ok(value) = std::env::var("QQBOT_LLM_REASONING_MODE") {
        config.reasoning_mode = match value.trim().to_ascii_lowercase().as_str() {
            "provider_default" => LlmReasoningMode::ProviderDefault,
            "qwen_no_think" => LlmReasoningMode::QwenNoThink,
            _ => {
                return Err(ConfigError::Invalid(
                    "QQBOT_LLM_REASONING_MODE must be provider_default or qwen_no_think".into(),
                ));
            }
        };
    }
    Ok(())
}

fn apply_qq_open_platform_env(config: &mut QqOpenPlatformConfig) -> Result<(), ConfigError> {
    apply_env_fields!(config;
        bool { enabled => "QQBOT_OPEN_PLATFORM_ENABLED" },
        non_empty {
            app_id => "QQBOT_OPEN_PLATFORM_APP_ID",
            owner_openid => "QQBOT_OPEN_PLATFORM_OWNER_OPENID",
        },
        path { client_secret_file => "QQBOT_OPEN_PLATFORM_CLIENT_SECRET_FILE" },
        positive {
            reconnect_initial_ms => "QQBOT_OPEN_PLATFORM_RECONNECT_INITIAL_MS",
            reconnect_max_ms => "QQBOT_OPEN_PLATFORM_RECONNECT_MAX_MS",
            notification_lease_secs => "QQBOT_OPEN_PLATFORM_NOTIFICATION_LEASE_SECS",
        },
    );
    Ok(())
}

fn apply_whitelist_env(config: &mut WhitelistConfig) -> Result<(), ConfigError> {
    apply_env_fields!(config;
        path { whitelist_file => "QQBOT_WHITELIST_FILE" },
    );
    Ok(())
}

fn parse_bool(name: &str, value: &str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::Invalid(format!(
            "{name} must be a boolean (true/false)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> Result<AppConfig, ConfigError> {
        let config: AppConfig = toml::from_str(content).map_err(|source| ConfigError::Parse {
            path: PathBuf::from("test.toml"),
            source,
        })?;
        config.validate(std::path::Path::new("."))?;
        Ok(config)
    }

    #[test]
    fn accepts_independent_qqbot_configuration() {
        let config = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
        )
        .unwrap();

        assert_eq!(config.napcat.self_qq_id, 12345);
        assert_eq!(config.napcat.reconnect_max_secs, 60);
        assert_eq!(config.database.max_connections, 5);
        assert_eq!(config.ingestion.queue_capacity, 1_024);
        assert!(config.thread_projection.enabled);
        assert_eq!(config.thread_projection.batch_size, 100);
        assert_eq!(config.thread_projection.same_conversation_window_secs, 300);
        assert!(config.thread_semantics.enabled);
        assert_eq!(config.thread_semantics.max_events, 50);
        assert_eq!(config.thread_semantics.max_total_chars, 50_000);
        assert!(config.thread_links.enabled);
        assert_eq!(config.thread_links.max_events, 100);
        assert_eq!(config.thread_links.max_total_chars, 100_000);
        assert!(!config.llm.enabled);
        assert_eq!(config.llm.base_url, "http://127.0.0.1:11434/v1");
    }

    #[test]
    fn unauthenticated_napcat_rejects_token_fields_and_non_loopback_urls() {
        let token_error = toml::from_str::<AppConfig>(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
http_token = "deprecated"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
        )
        .unwrap_err();
        assert!(token_error.to_string().contains("unknown field"));

        let remote_error = parse(
            r#"
[napcat]
ws_url = "ws://192.0.2.10:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
        )
        .unwrap_err();
        assert!(remote_error.to_string().contains("loopback host"));

        let query_token_error = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700?access_token=deprecated"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
        )
        .unwrap_err();
        assert!(query_token_error.to_string().contains("must not contain"));
    }

    #[test]
    fn accepts_bounded_loopback_llm_configuration() {
        let config = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[llm]
enabled = true
base_url = "http://127.0.0.1:11434/v1"
model = "qwen3:8b"
reasoning_mode = "qwen_no_think"
max_input_chars = 50000
max_output_tokens = 1500
max_candidates_per_kind = 10
"#,
        )
        .unwrap();
        assert!(config.llm.enabled);
        assert_eq!(config.llm.model, "qwen3:8b");
        assert_eq!(config.llm.reasoning_mode, LlmReasoningMode::QwenNoThink);
    }

    #[test]
    fn rejects_llm_secret_in_toml_and_remote_plain_http() {
        let secret_error = toml::from_str::<AppConfig>(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[llm]
enabled = true
base_url = "https://api.example.com/v1"
model = "model"
api_key = "must-not-be-accepted"
"#,
        )
        .unwrap_err();
        assert!(secret_error.to_string().contains("unknown field"));

        let transport_error = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[llm]
enabled = true
base_url = "http://192.0.2.10:8000/v1"
model = "model"
"#,
        )
        .unwrap_err();
        assert!(transport_error.to_string().contains("loopback"));
    }

    #[test]
    fn rejects_unknown_business_configuration() {
        let error = toml::from_str::<AppConfig>(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[business]
auto_reply = true
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn official_qq_secret_cannot_be_written_in_toml() {
        let error = toml::from_str::<AppConfig>(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[qq_open_platform]
enabled = true
app_id = "test-app"
owner_openid = "test-owner"
client_secret = "must-not-be-accepted"
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn official_qq_owner_openid_respects_database_boundary() {
        let config = QqOpenPlatformConfig {
            enabled: true,
            app_id: "test-app".into(),
            owner_openid: "x".repeat(192),
            client_secret_file: Some(PathBuf::from("unused-in-validation")),
            ..QqOpenPlatformConfig::default()
        };
        let error = config.validate().unwrap_err();
        assert!(error.to_string().contains("1..=191"));
    }

    #[test]
    fn rejects_non_websocket_listener_url() {
        let error = parse(
            r#"
[napcat]
ws_url = "http://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("napcat.ws_url"));
    }

    #[test]
    fn rejects_zero_or_unbounded_ingestion_configuration() {
        let zero_capacity = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[ingestion]
queue_capacity = 0
"#,
        )
        .unwrap_err();
        assert!(zero_capacity.to_string().contains("queue_capacity"));

        let unbounded_capacity = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[ingestion]
queue_capacity = 65537
"#,
        )
        .unwrap_err();
        assert!(unbounded_capacity.to_string().contains("queue_capacity"));
    }

    #[test]
    fn default_backfill_configuration_loads_and_validates() {
        let config = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
        )
        .unwrap();

        assert!(config.backfill.enabled);
        assert_eq!(config.backfill.page_size, 100);
        assert_eq!(config.backfill.max_concurrency, 2);
        // 默认配置必须能构造出合法的有界预算。
        assert!(config.backfill.budget().is_ok());
    }

    #[test]
    fn backfill_page_size_must_be_between_1_and_100() {
        let too_small = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
page_size = 0
"#,
        )
        .unwrap_err();
        assert!(too_small.to_string().contains("page_size"));

        let too_large = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
page_size = 101
"#,
        )
        .unwrap_err();
        assert!(too_large.to_string().contains("page_size"));
    }

    #[test]
    fn backfill_zero_or_over_limit_fields_are_rejected() {
        let zero_concurrency = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
max_concurrency = 0
"#,
        )
        .unwrap_err();
        assert!(zero_concurrency.to_string().contains("max_concurrency"));

        let over_limit_concurrency = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
max_concurrency = 65
"#,
        )
        .unwrap_err();
        assert!(
            over_limit_concurrency
                .to_string()
                .contains("max_concurrency")
        );
    }

    #[test]
    fn backfill_retry_max_below_initial_is_rejected() {
        let error = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
retry_initial_ms = 1000
retry_max_ms = 500
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("retry_max_ms"));
    }

    #[test]
    fn backfill_unknown_field_is_rejected() {
        let error = toml::from_str::<AppConfig>(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
auto_mark_complete = true
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn thread_projection_bounds_and_unknown_fields_are_rejected() {
        let invalid_window = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[thread_projection]
same_conversation_window_secs = 0
"#,
        )
        .unwrap_err();
        assert!(
            invalid_window
                .to_string()
                .contains("same_conversation_window_secs")
        );

        let unknown = toml::from_str::<AppConfig>(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[thread_projection]
llm_per_message = true
"#,
        )
        .unwrap_err();
        assert!(unknown.to_string().contains("unknown field"));
    }

    #[test]
    fn thread_semantics_bounds_and_unknown_fields_are_rejected() {
        let invalid_budget = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[thread_semantics]
max_total_chars = 0
"#,
        )
        .unwrap_err();
        assert!(invalid_budget.to_string().contains("max_total_chars"));

        let unknown = toml::from_str::<AppConfig>(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[thread_semantics]
trust_model_output = true
"#,
        )
        .unwrap_err();
        assert!(unknown.to_string().contains("unknown field"));
    }

    #[test]
    fn follow_up_bounds_and_unknown_fields_are_rejected() {
        let invalid = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[follow_up]
scan_interval_ms = 999
"#,
        )
        .unwrap_err();
        assert!(invalid.to_string().contains("follow_up.scan_interval_ms"));

        let unknown = toml::from_str::<AppConfig>(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[follow_up]
send_via_napcat = true
"#,
        )
        .unwrap_err();
        assert!(unknown.to_string().contains("unknown field"));
    }

    #[test]
    fn backfill_env_overrides_only_use_qqbot_prefix() {
        // 通过环境变量覆盖 page_size 并校验；保证只使用 QQBOT_BACKFILL_* 前缀。
        // SAFETY: 单线程单元测试，仅在测试期间临时设置后立即移除。
        unsafe {
            std::env::set_var("QQBOT_BACKFILL_PAGE_SIZE", "50");
        }
        let config: AppConfig = toml::from_str(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
        )
        .unwrap();
        let mut config = config;
        config.apply_env_overrides().unwrap();
        config.validate(std::path::Path::new(".")).unwrap();
        assert_eq!(config.backfill.page_size, 50);
        // SAFETY: 同上，测试结束清理。
        unsafe {
            std::env::remove_var("QQBOT_BACKFILL_PAGE_SIZE");
        }
    }

    // ===== 白名单单元测试 =====

    use std::io::Write;

    /// 创建临时白名单 JSON 文件，返回文件路径。
    fn write_whitelist_json(groups: &[i64]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("create temp file");
        let json = serde_json::json!({ "groups": groups });
        write!(file, "{json}").expect("write temp file");
        file
    }

    #[test]
    fn whitelist_loads_allowed_groups() {
        let file = write_whitelist_json(&[671260344, 123456]);
        let config = WhitelistConfig {
            whitelist_file: Some(file.path().to_path_buf()),
        };
        let groups = config.load_groups(std::path::Path::new(".")).unwrap();
        assert!(groups.contains(&671260344));
        assert!(groups.contains(&123456));
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn whitelist_rejects_non_listed_group() {
        let file = write_whitelist_json(&[671260344]);
        let config = WhitelistConfig {
            whitelist_file: Some(file.path().to_path_buf()),
        };
        let groups = config.load_groups(std::path::Path::new(".")).unwrap();
        assert!(groups.contains(&671260344));
        // 非白名单群不在集合中
        assert!(!groups.contains(&999999999));
    }

    #[test]
    fn whitelist_empty_config_means_no_filtering() {
        // 不配 whitelist_file 时返回空集合，表示不启用白名单（放行所有群）。
        let config = WhitelistConfig::default();
        let groups = config.load_groups(std::path::Path::new(".")).unwrap();
        assert!(groups.is_empty());
    }

    #[test]
    fn whitelist_deduplicates_repeated_group_ids() {
        // 重复群号应该去重。
        let file = write_whitelist_json(&[671260344, 671260344, 123456]);
        let config = WhitelistConfig {
            whitelist_file: Some(file.path().to_path_buf()),
        };
        let groups = config.load_groups(std::path::Path::new(".")).unwrap();
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn whitelist_rejects_empty_groups_array() {
        let file = write_whitelist_json(&[]);
        let config = WhitelistConfig {
            whitelist_file: Some(file.path().to_path_buf()),
        };
        let result = config.validate(std::path::Path::new("."));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("不能为空"));
    }

    #[test]
    fn whitelist_rejects_nonexistent_file() {
        let config = WhitelistConfig {
            whitelist_file: Some(PathBuf::from("/nonexistent/whitelist.json")),
        };
        let result = config.validate(std::path::Path::new("."));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("不存在"));
    }

    #[test]
    fn whitelist_rejects_invalid_json() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{{invalid json}}").unwrap();
        let config = WhitelistConfig {
            whitelist_file: Some(file.path().to_path_buf()),
        };
        let result = config.validate(std::path::Path::new("."));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("JSON 格式错误"));
    }

    #[test]
    fn whitelist_resolves_relative_path_from_config_dir() {
        // 相对路径应以 config_dir 为基准。
        let file = write_whitelist_json(&[671260344]);
        let file_name = file
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let dir = file.path().parent().unwrap().to_path_buf();
        let config = WhitelistConfig {
            whitelist_file: Some(PathBuf::from(file_name)),
        };
        let groups = config.load_groups(&dir).unwrap();
        assert!(groups.contains(&671260344));
    }

    #[test]
    fn whitelist_absolute_path_ignores_config_dir() {
        let file = write_whitelist_json(&[671260344]);
        let config = WhitelistConfig {
            whitelist_file: Some(file.path().to_path_buf()),
        };
        // 绝对路径不应受 config_dir 影响。
        let groups = config
            .load_groups(std::path::Path::new("/some/other/dir"))
            .unwrap();
        assert!(groups.contains(&671260344));
    }
}
