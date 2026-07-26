//! NapCat WebSocket/HTTP 连接配置（无 Token 本机模式）。
//!
//! 不得重新加入 `NAPCAT_HTTP_TOKEN`、WebSocket Token 或 URL 查询凭据。

use serde::Deserialize;

use qqbot::napcat::HeartbeatConfig;

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
    /// OneBot Heartbeat 超时监控配置（评审第三轮 P1-3）。
    /// 缺失 `[napcat.heartbeat]` 时使用 `HeartbeatConfig::default()`（宽松、启用）。
    /// 可通过 TOML 调整启动宽限、超时倍数，或 `enabled=false` 禁用 watchdog。
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,
}

fn default_reconnect_initial_secs() -> u64 {
    1
}

fn default_reconnect_max_secs() -> u64 {
    60
}
