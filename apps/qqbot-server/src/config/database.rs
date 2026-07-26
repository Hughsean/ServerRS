//! QQBot 独立 MySQL 连接配置。

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default = "default_database_max_connections")]
    pub max_connections: u32,
}

fn default_database_max_connections() -> u32 {
    5
}
