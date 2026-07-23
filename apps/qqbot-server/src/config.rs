use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub napcat: NapCatConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub ingestion: IngestionConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NapCatConfig {
    pub ws_url: String,
    pub http_base_url: String,
    #[serde(default)]
    pub http_token: String,
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

impl AppConfig {
    /// QQBot 使用应用目录内的独立配置入口，不读取数字人的根 `.env` 或 `CONFIG_PATH`。
    pub fn load() -> Result<Self, ConfigError> {
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
        config.validate()?;
        Ok(config)
    }

    fn apply_env_overrides(&mut self) -> Result<(), ConfigError> {
        if let Ok(value) = std::env::var("NAPCAT_WS_URL")
            && !value.trim().is_empty()
        {
            self.napcat.ws_url = value;
        }
        if let Ok(value) = std::env::var("NAPCAT_HTTP_BASE_URL")
            && !value.trim().is_empty()
        {
            self.napcat.http_base_url = value;
        }
        if let Ok(value) = std::env::var("NAPCAT_HTTP_TOKEN") {
            self.napcat.http_token = value;
        }
        if let Ok(value) = std::env::var("NAPCAT_SELF_QQ_ID") {
            self.napcat.self_qq_id = value.parse().map_err(|_| {
                ConfigError::Invalid("NAPCAT_SELF_QQ_ID must be a positive integer".into())
            })?;
        }
        if let Ok(value) = std::env::var("QQBOT_DATABASE_URL")
            && !value.trim().is_empty()
        {
            self.database.url = value;
        }
        if let Ok(value) = std::env::var("QQBOT_DATABASE_MAX_CONNECTIONS") {
            self.database.max_connections = value.parse().map_err(|_| {
                ConfigError::Invalid(
                    "QQBOT_DATABASE_MAX_CONNECTIONS must be a positive integer".into(),
                )
            })?;
        }
        if let Ok(value) = std::env::var("QQBOT_INGESTION_QUEUE_CAPACITY") {
            self.ingestion.queue_capacity =
                parse_positive("QQBOT_INGESTION_QUEUE_CAPACITY", &value)?;
        }
        if let Ok(value) = std::env::var("QQBOT_INGESTION_RETRY_INITIAL_MS") {
            self.ingestion.retry_initial_ms =
                parse_positive("QQBOT_INGESTION_RETRY_INITIAL_MS", &value)?;
        }
        if let Ok(value) = std::env::var("QQBOT_INGESTION_RETRY_MAX_MS") {
            self.ingestion.retry_max_ms = parse_positive("QQBOT_INGESTION_RETRY_MAX_MS", &value)?;
        }
        if let Ok(value) = std::env::var("QQBOT_INGESTION_SHUTDOWN_DRAIN_TIMEOUT_SECS") {
            self.ingestion.shutdown_drain_timeout_secs =
                parse_positive("QQBOT_INGESTION_SHUTDOWN_DRAIN_TIMEOUT_SECS", &value)?;
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), ConfigError> {
        validate_url(&self.napcat.ws_url, &["ws", "wss"], "napcat.ws_url")?;
        validate_url(
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
    T: std::str::FromStr + PartialEq + Default,
{
    let parsed = value
        .parse::<T>()
        .map_err(|_| ConfigError::Invalid(format!("{name} must be a positive integer")))?;
    if parsed == T::default() {
        return Err(ConfigError::Invalid(format!(
            "{name} must be a positive integer"
        )));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> Result<AppConfig, ConfigError> {
        let config: AppConfig = toml::from_str(content).map_err(|source| ConfigError::Parse {
            path: PathBuf::from("test.toml"),
            source,
        })?;
        config.validate()?;
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
}
