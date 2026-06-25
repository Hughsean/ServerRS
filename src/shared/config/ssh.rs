use serde::Deserialize;
use std::collections::HashMap;

/// SSH 隧道方向
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelDirection {
    Local,
    Remote,
}

impl Default for TunnelDirection {
    fn default() -> Self {
        Self::Local
    }
}

/// SSH 隧道配置，对应 config.toml 中的 [ssh_tunnels.*] 块。
#[derive(Debug, Clone, Deserialize)]
pub struct SshTunnelConfig {
    /// 跳板机地址（支持 ~/.ssh/config 中的 Host 别名）
    pub host: String,
    /// SSH 登录用户名（可选；不填时直接用 host 地址，
    /// 适用于 ~/.ssh/config 中已定义 User 的情况）
    pub user: Option<String>,
    /// 本地监听端口
    pub local_port: u16,
    /// 转发到远程的端口
    pub remote_port: u16,
    /// 隧道方向（默认 Local）
    #[serde(default)]
    pub direction: TunnelDirection,
    /// 远程端绑定地址（仅对 -R 有效；不填时绑定 127.0.0.1）
    pub bind_address: Option<String>,
}

/// 所有 SSH 隧道定义，key 为隧道名称
pub type SshTunnelMap = HashMap<String, SshTunnelConfig>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direction_default_is_local() {
        let dir = TunnelDirection::default();
        assert!(matches!(dir, TunnelDirection::Local));
    }

    #[test]
    fn test_ssh_config_default_direction_is_local() {
        let toml_str = r#"
            host = "test"
            local_port = 8080
            remote_port = 9090
        "#;
        let config: SshTunnelConfig = toml::from_str(toml_str).unwrap();
        assert!(matches!(config.direction, TunnelDirection::Local));
        assert!(config.bind_address.is_none());
    }

    #[test]
    fn test_ssh_config_remote_direction() {
        let toml_str = r#"
            host = "test"
            local_port = 8080
            remote_port = 9090
            direction = "remote"
            bind_address = "0.0.0.0"
        "#;
        let config: SshTunnelConfig = toml::from_str(toml_str).unwrap();
        assert!(matches!(config.direction, TunnelDirection::Remote));
        assert_eq!(config.bind_address.as_deref(), Some("0.0.0.0"));
    }
}
