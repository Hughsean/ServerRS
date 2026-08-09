//! 本机管理员页面配置。密码只允许来自环境变量，不进入 TOML 或日志。

use std::net::IpAddr;

use serde::Deserialize;

use super::ConfigError;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AdminConfig {
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
    pub session_ttl_secs: u64,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: "127.0.0.1".into(),
            port: 8080,
            session_ttl_secs: 8 * 60 * 60,
        }
    }
}

impl AdminConfig {
    pub(super) fn validate(&self, whitelist_configured: bool) -> Result<(), ConfigError> {
        if !self.enabled {
            return Ok(());
        }
        let address = self
            .bind
            .parse::<IpAddr>()
            .map_err(|_| ConfigError::Invalid("admin.bind must be an IP address".into()))?;
        if !address.is_loopback() && !address.is_unspecified() {
            return Err(ConfigError::Invalid(
                "admin.bind must be loopback or an unspecified container address".into(),
            ));
        }
        if self.port == 0 {
            return Err(ConfigError::Invalid("admin.port must be positive".into()));
        }
        if !(300..=86_400).contains(&self.session_ttl_secs) {
            return Err(ConfigError::Invalid(
                "admin.session_ttl_secs must be in 300..=86400".into(),
            ));
        }
        if !whitelist_configured {
            return Err(ConfigError::Invalid(
                "enabled admin UI requires whitelist.whitelist_file".into(),
            ));
        }
        if std::env::var("QQBOT_ADMIN_PASSWORD")
            .ok()
            .filter(|value| value.len() >= 12 && value.len() <= 256)
            .is_none()
        {
            return Err(ConfigError::Invalid(
                "enabled admin UI requires QQBOT_ADMIN_PASSWORD with 12..=256 bytes".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn password(&self) -> Result<String, ConfigError> {
        self.validate(true)?;
        std::env::var("QQBOT_ADMIN_PASSWORD")
            .map_err(|_| ConfigError::Invalid("QQBOT_ADMIN_PASSWORD is unavailable".into()))
    }
}
