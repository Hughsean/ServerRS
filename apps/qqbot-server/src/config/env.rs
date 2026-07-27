//! 环境变量覆盖与解析助手。
//!
//! 环境变量覆盖只保留四种常见、类型明确的机械操作（bool/positive/non_empty/path）。
//! 特殊枚举和浮点语义仍在调用处显式解析，避免形成难以调试的万能配置 DSL。
//! 宏中引用的 `parse_bool`/`parse_positive`/`PathBuf`/`ConfigError` 按定义点作用域解析。

use std::path::PathBuf;

use super::ConfigError;
use super::llm::{LlmConfig, LlmReasoningMode};
use super::qq_open_platform::QqOpenPlatformConfig;
use super::whitelist::WhitelistConfig;
use super::workers::{
    ArtifactConfig, BackfillConfig, FollowUpConfig, HealthConfig, RecallWalConfig,
    ThreadLinksConfig, ThreadProjectionConfig, ThreadSemanticsConfig,
};

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

pub(super) fn parse_positive<T>(name: &str, value: &str) -> Result<T, ConfigError>
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

pub(super) fn parse_bool(name: &str, value: &str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::Invalid(format!(
            "{name} must be a boolean (true/false)"
        ))),
    }
}

/// 应用 `QQBOT_BACKFILL_*` 环境变量覆盖。只使用 QQBot 专属前缀，不读取数字人配置。
pub(super) fn apply_backfill_env(config: &mut BackfillConfig) -> Result<(), ConfigError> {
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

pub(super) fn apply_thread_projection_env(
    config: &mut ThreadProjectionConfig,
) -> Result<(), ConfigError> {
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

pub(super) fn apply_thread_semantics_env(
    config: &mut ThreadSemanticsConfig,
) -> Result<(), ConfigError> {
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

pub(super) fn apply_thread_links_env(config: &mut ThreadLinksConfig) -> Result<(), ConfigError> {
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

pub(super) fn apply_follow_up_env(config: &mut FollowUpConfig) -> Result<(), ConfigError> {
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

pub(super) fn apply_artifact_env(config: &mut ArtifactConfig) -> Result<(), ConfigError> {
    apply_env_fields!(config;
        bool { enabled => "QQBOT_ARTIFACT_ENABLED" },
        positive {
            default_ttl_secs => "QQBOT_ARTIFACT_DEFAULT_TTL_SECS",
            ttl_scan_interval_ms => "QQBOT_ARTIFACT_TTL_SCAN_INTERVAL_MS",
        },
    );
    Ok(())
}

pub(super) fn apply_recall_wal_env(config: &mut RecallWalConfig) -> Result<(), ConfigError> {
    if let Ok(value) = std::env::var("QQBOT_RECALL_WAL_PATH")
        && !value.trim().is_empty()
    {
        config.path = PathBuf::from(value);
    }
    if let Ok(value) = std::env::var("QQBOT_RECALL_WAL_QUARANTINE_DIR")
        && !value.trim().is_empty()
    {
        config.quarantine_dir = PathBuf::from(value);
    }
    if let Ok(value) = std::env::var("QQBOT_RECALL_WAL_KEY_ENV")
        && !value.trim().is_empty()
    {
        config.key_env = value;
    }
    apply_env_fields!(config;
        positive {
            max_bytes => "QQBOT_RECALL_WAL_MAX_BYTES",
            drain_interval_ms => "QQBOT_RECALL_WAL_DRAIN_INTERVAL_MS",
        },
    );
    Ok(())
}

pub(super) fn apply_health_env(config: &mut HealthConfig) -> Result<(), ConfigError> {
    apply_env_fields!(config;
        bool { enabled => "QQBOT_HEALTH_ENABLED" },
        positive {
            cache_ttl_secs => "QQBOT_HEALTH_CACHE_TTL_SECS",
            log_interval_ms => "QQBOT_HEALTH_LOG_INTERVAL_MS",
            worker_success_stale_secs => "QQBOT_HEALTH_WORKER_SUCCESS_STALE_SECS",
        },
    );
    Ok(())
}

pub(super) fn apply_llm_env(config: &mut LlmConfig) -> Result<(), ConfigError> {
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

pub(super) fn apply_qq_open_platform_env(
    config: &mut QqOpenPlatformConfig,
) -> Result<(), ConfigError> {
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

pub(super) fn apply_whitelist_env(config: &mut WhitelistConfig) -> Result<(), ConfigError> {
    apply_env_fields!(config;
        path { whitelist_file => "QQBOT_WHITELIST_FILE" },
    );
    Ok(())
}
