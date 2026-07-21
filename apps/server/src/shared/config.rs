//! Process-level configuration aggregation.
//!
//! Digital-human settings are owned and validated by `digital-human`; the
//! optional QQ section is owned by `qqbot`. This host combines both without
//! introducing a dependency between the business crates.

use std::ops::{Deref, DerefMut};

use digital_human::shared::config::AppConfig as DigitalHumanConfig;

pub use digital_human::shared::config::{
    AuthConfig, FreshContextConfig, JwtConfig, WebIngestionConfig,
};

#[derive(Debug, Clone)]
pub struct AppConfig {
    digital_human: DigitalHumanConfig,
    #[cfg(feature = "qq_bot")]
    pub qq_bot: qqbot::QqBotConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            digital_human: DigitalHumanConfig::default(),
            #[cfg(feature = "qq_bot")]
            qq_bot: qqbot::QqBotConfig::default(),
        }
    }
}

impl Deref for AppConfig {
    type Target = DigitalHumanConfig;

    fn deref(&self) -> &Self::Target {
        &self.digital_human
    }
}

impl DerefMut for AppConfig {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.digital_human
    }
}

impl AppConfig {
    pub fn load() -> Self {
        let _ = dotenvy::dotenv();

        let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".into());
        let mut config = match std::fs::read_to_string(&path) {
            Ok(content) => Self::parse(&content).unwrap_or_else(|error| {
                panic!("failed to parse configuration file {path}: {error}")
            }),
            Err(error) => {
                tracing::warn!(path = %path, error = %error, "未找到配置文件，使用默认配置");
                Self::default()
            }
        };

        config.digital_human.apply_env_overrides();
        config.apply_qq_env_overrides();
        config
            .digital_human
            .resolve_tunnel_templates()
            .unwrap_or_else(|error| panic!("invalid application configuration: {error}"));
        config
            .digital_human
            .validate()
            .unwrap_or_else(|error| panic!("invalid application configuration: {error}"));
        config
    }

    fn parse(content: &str) -> Result<Self, String> {
        let mut root: toml::Value = toml::from_str(content).map_err(|error| error.to_string())?;
        let table = root
            .as_table_mut()
            .ok_or_else(|| "configuration root must be a TOML table".to_string())?;
        let qq_value = table.remove("qq_bot");
        let digital_human = root
            .try_into::<DigitalHumanConfig>()
            .map_err(|error| error.to_string())?;

        #[cfg(feature = "qq_bot")]
        let qq_bot = match qq_value {
            Some(value) => value
                .try_into::<qqbot::QqBotConfig>()
                .map_err(|error| error.to_string())?,
            None => qqbot::QqBotConfig::default(),
        };

        #[cfg(not(feature = "qq_bot"))]
        let _ = qq_value;

        Ok(Self {
            digital_human,
            #[cfg(feature = "qq_bot")]
            qq_bot,
        })
    }

    fn apply_qq_env_overrides(&mut self) {
        #[cfg(feature = "qq_bot")]
        {
            if let Ok(value) = std::env::var("QQ_BOT_TTS_OUTPUT_DIR")
                && !value.is_empty()
            {
                self.qq_bot.tts_output_dir = value;
            }
            if let Ok(value) = std::env::var("QQ_BOT_TTS_PUBLIC_URL_BASE")
                && !value.is_empty()
            {
                self.qq_bot.tts_public_url_base = value;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_strips_optional_qq_section_before_digital_human_deserialization() {
        let config = AppConfig::parse(
            r#"
[qq_bot]
enabled = true
self_qq_id = 42
"#,
        )
        .expect("host configuration must parse");

        #[cfg(feature = "qq_bot")]
        {
            assert!(config.qq_bot.enabled);
            assert_eq!(config.qq_bot.self_qq_id, 42);
        }

        #[cfg(not(feature = "qq_bot"))]
        let _ = config;
    }
}
