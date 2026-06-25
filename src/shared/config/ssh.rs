use serde::Deserialize;
use std::collections::HashMap;

/// SSH 隧道配置，对应 config.toml 中的 [ssh_tunnels.*] 块。
#[derive(Debug, Clone, Deserialize)]
pub struct SshTunnelConfig {
    /// 跳板机地址
    pub host: String,
    /// SSH 登录用户名
    pub user: String,
    /// 本地监听端口
    pub local_port: u16,
    /// 转发到远程的端口
    pub remote_port: u16,
}

/// 所有 SSH 隧道定义，key 为隧道名称
pub type SshTunnelMap = HashMap<String, SshTunnelConfig>;
