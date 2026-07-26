//! 官方 QQ Bot 通道配置。
//!
//! Secret 只允许来自进程环境或本地文件，不接受 TOML 明文字段。
//! 本任务范围不启用 QQ 开放平台通道。

use std::path::PathBuf;

use qq_open_platform::QqBotCredentials;
use serde::Deserialize;

use super::ConfigError;

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
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
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
