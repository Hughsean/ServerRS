//! `AppConfig`：QQBot 应用配置根结构、加载入口、环境变量覆盖与校验编排。
//!
//! QQBot 使用应用目录内的独立配置入口，不读取数字人的根 `.env` 或 `CONFIG_PATH`。
//! 相对路径继续以配置文件目录为基准。

use std::path::PathBuf;

use serde::Deserialize;

use super::ConfigError;
use super::action_planner::ActionPlannerConfig;
use super::database::DatabaseConfig;
use super::env::{
    apply_agenda_env, apply_artifact_env, apply_backfill_env, apply_follow_up_env,
    apply_health_env, apply_llm_env, apply_memory_candidates_env, apply_notification_policy_env,
    apply_qq_open_platform_env, apply_recall_wal_env, apply_thread_links_env,
    apply_thread_projection_env, apply_thread_semantics_env, apply_whitelist_env, parse_bool,
    parse_positive,
};
use super::llm::LlmConfig;
use super::napcat::NapCatConfig;
use super::qq_open_platform::QqOpenPlatformConfig;
use super::validation::{is_loopback_host, validate_loopback_url, validate_url};
use super::whitelist::WhitelistConfig;
use super::workers::{
    AgendaConfig, ArtifactConfig, BackfillConfig, DirectorySyncConfig, FollowUpConfig,
    HealthConfig, IngestionConfig, MemoryCandidatesConfig, NotificationPolicyConfig,
    RecallWalConfig, ReplyReconcileConfig, ThreadLinksConfig, ThreadProjectionConfig,
    ThreadSemanticsConfig,
};

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
    pub reply_reconcile: ReplyReconcileConfig,
    #[serde(default)]
    pub thread_projection: ThreadProjectionConfig,
    #[serde(default)]
    pub thread_semantics: ThreadSemanticsConfig,
    #[serde(default)]
    pub thread_links: ThreadLinksConfig,
    #[serde(default)]
    pub follow_up: FollowUpConfig,
    #[serde(default)]
    pub agenda: AgendaConfig,
    #[serde(default)]
    pub notification_policy: NotificationPolicyConfig,
    #[serde(default)]
    pub directory_sync: DirectorySyncConfig,
    #[serde(default)]
    pub artifact: ArtifactConfig,
    #[serde(default)]
    pub recall_wal: RecallWalConfig,
    #[serde(default)]
    pub health: HealthConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub qq_open_platform: QqOpenPlatformConfig,
    #[serde(default)]
    pub whitelist: WhitelistConfig,
    #[serde(default)]
    pub action_planner: ActionPlannerConfig,
    #[serde(default)]
    pub memory_candidates: MemoryCandidatesConfig,
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
        config.resolve_relative_paths(&config_dir);
        config.validate(&config_dir)?;
        Ok((config, config_dir))
    }

    pub(super) fn apply_env_overrides(&mut self) -> Result<(), ConfigError> {
        apply_env_fields!(&mut self.napcat;
            non_empty {
                ws_url => "NAPCAT_WS_URL",
                http_base_url => "NAPCAT_HTTP_BASE_URL",
            },
            positive { self_qq_id => "NAPCAT_SELF_QQ_ID" },
        );
        // 评审第三轮 P1-3：Heartbeat 配置环境变量覆盖。
        // 允许通过 NAPCAT_HEARTBEAT_* 调整启动宽限、超时倍数或禁用 watchdog，
        // 适配不发送兼容心跳的 NapCat 实现。
        apply_env_fields!(&mut self.napcat.heartbeat;
            bool { enabled => "NAPCAT_HEARTBEAT_ENABLED" },
            positive {
                startup_grace_secs => "NAPCAT_HEARTBEAT_STARTUP_GRACE_SECS",
                min_interval_secs => "NAPCAT_HEARTBEAT_MIN_INTERVAL_SECS",
                max_interval_secs => "NAPCAT_HEARTBEAT_MAX_INTERVAL_SECS",
                default_interval_secs => "NAPCAT_HEARTBEAT_DEFAULT_INTERVAL_SECS",
                timeout_multiplier => "NAPCAT_HEARTBEAT_TIMEOUT_MULTIPLIER",
            },
        );
        apply_env_fields!(&mut self.database;
            non_empty { url => "QQBOT_DATABASE_URL" },
            positive { max_connections => "QQBOT_DATABASE_MAX_CONNECTIONS" },
        );
        apply_env_fields!(&mut self.ingestion;
            positive {
                queue_capacity => "QQBOT_INGESTION_QUEUE_CAPACITY",
                batch_size => "QQBOT_INGESTION_BATCH_SIZE",
                batch_flush_ms => "QQBOT_INGESTION_BATCH_FLUSH_MS",
                retry_initial_ms => "QQBOT_INGESTION_RETRY_INITIAL_MS",
                retry_max_ms => "QQBOT_INGESTION_RETRY_MAX_MS",
                shutdown_drain_timeout_secs => "QQBOT_INGESTION_SHUTDOWN_DRAIN_TIMEOUT_SECS",
            },
        );
        apply_backfill_env(&mut self.backfill)?;
        apply_thread_projection_env(&mut self.thread_projection)?;
        apply_thread_semantics_env(&mut self.thread_semantics)?;
        apply_memory_candidates_env(&mut self.memory_candidates)?;
        apply_thread_links_env(&mut self.thread_links)?;
        apply_follow_up_env(&mut self.follow_up)?;
        apply_agenda_env(&mut self.agenda)?;
        apply_notification_policy_env(&mut self.notification_policy)?;
        apply_artifact_env(&mut self.artifact)?;
        apply_recall_wal_env(&mut self.recall_wal)?;
        apply_health_env(&mut self.health)?;
        apply_llm_env(&mut self.llm)?;
        apply_env_fields!(&mut self.action_planner;
            bool { enabled => "QQBOT_ACTION_PLANNER_ENABLED" },
            positive {
                max_batches_per_scan => "QQBOT_ACTION_PLANNER_MAX_BATCHES_PER_SCAN",
                lease_secs => "QQBOT_ACTION_PLANNER_LEASE_SECS",
                scan_interval_ms => "QQBOT_ACTION_PLANNER_SCAN_INTERVAL_MS",
                retry_initial_ms => "QQBOT_ACTION_PLANNER_RETRY_INITIAL_MS",
                retry_max_ms => "QQBOT_ACTION_PLANNER_RETRY_MAX_MS",
            },
        );
        apply_qq_open_platform_env(&mut self.qq_open_platform)?;
        apply_whitelist_env(&mut self.whitelist)?;
        Ok(())
    }

    /// 记忆提取所调用的 LLM 端点是否已验证为回环地址。
    /// 决定 `local_only` 内容信任等级是否可进入记忆候选提取：NapCat 端点固定为
    /// 回环，但 LLM 端点可配置为远程地址，`local_only` 正文绝不能发送给远程模型，
    /// 因此信任判定的对象是 `llm.base_url` 而非 NapCat 端点。
    pub fn llm_endpoint_verified_loopback(&self) -> bool {
        url::Url::parse(&self.llm.base_url)
            .ok()
            .and_then(|url| url.host_str().map(is_loopback_host))
            .unwrap_or(false)
    }

    fn resolve_relative_paths(&mut self, config_dir: &std::path::Path) {
        if self.recall_wal.path.is_relative() {
            self.recall_wal.path = config_dir.join(&self.recall_wal.path);
        }
        if self.recall_wal.quarantine_dir.is_relative() {
            self.recall_wal.quarantine_dir = config_dir.join(&self.recall_wal.quarantine_dir);
        }
    }

    pub(super) fn validate(&self, config_dir: &std::path::Path) -> Result<(), ConfigError> {
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
        // 评审第三轮 P1-3：校验 Heartbeat 配置边界，拒绝 0/溢出/异常巨大值。
        self.napcat
            .heartbeat
            .validate()
            .map_err(|e| ConfigError::Invalid(format!("napcat.heartbeat: {e}")))?;
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
        if !(1..=500).contains(&self.ingestion.batch_size) {
            return Err(ConfigError::Invalid(
                "ingestion.batch_size must be between 1 and 500".into(),
            ));
        }
        if !(1..=1000).contains(&self.ingestion.batch_flush_ms) {
            return Err(ConfigError::Invalid(
                "ingestion.batch_flush_ms must be between 1 and 1000".into(),
            ));
        }
        if self.ingestion.queue_capacity < self.ingestion.batch_size {
            return Err(ConfigError::Invalid(
                "ingestion.queue_capacity must be >= batch_size".into(),
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
        self.reply_reconcile.validate()?;
        self.thread_projection.validate()?;
        self.thread_semantics.validate()?;
        self.thread_links.validate()?;
        self.follow_up.validate()?;
        self.agenda.validate()?;
        self.notification_policy.validate()?;
        self.directory_sync.validate()?;
        self.artifact.validate()?;
        self.recall_wal.validate()?;
        self.health.validate()?;
        self.llm.validate()?;
        self.qq_open_platform.validate()?;
        self.whitelist.validate(config_dir)?;
        self.action_planner.validate()?;
        self.memory_candidates.validate()?;
        Ok(())
    }
}
