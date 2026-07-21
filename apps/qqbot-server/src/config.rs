use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub napcat: NapCatConfig,
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
    /// QQBot 使用独立配置入口，不读取数字人服务器的 CONFIG_PATH。
    pub fn load() -> Result<Self, ConfigError> {
        let _ = dotenvy::dotenv();
        let path = std::env::var_os("QQBOT_CONFIG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("qqbot.toml"));
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
    fn accepts_protocol_only_configuration() {
        let config = parse(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345
"#,
        )
        .unwrap();

        assert_eq!(config.napcat.self_qq_id, 12345);
        assert_eq!(config.napcat.reconnect_max_secs, 60);
    }

    #[test]
    fn rejects_database_or_business_configuration() {
        let error = toml::from_str::<AppConfig>(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://should-not-be-accepted"
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
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("napcat.ws_url"));
    }
}
