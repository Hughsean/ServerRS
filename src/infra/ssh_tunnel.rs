use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::info;

use crate::shared::config::{SshTunnelConfig, TunnelDirection};

/// 根据隧道方向构建 -L 或 -R 参数。
///
///   Local:  -L [bind_address:]local_port:127.0.0.1:remote_port
///   Remote: -R [bind_address:]remote_port:127.0.0.1:local_port
fn build_forward_spec(config: &SshTunnelConfig) -> String {
    let host = "127.0.0.1";
    let spec = match config.direction {
        TunnelDirection::Local => {
            format!("{}:{}:{}", config.local_port, host, config.remote_port)
        }
        TunnelDirection::Remote => {
            format!("{}:{}:{}", config.remote_port, host, config.local_port)
        }
    };
    match &config.bind_address {
        Some(addr) => format!("{}:{}", addr, spec),
        None => spec,
    }
}

/// 单个 SSH 隧道实例，持有 ssh 子进程句柄。
struct SshTunnel {
    name: String,
    _config: SshTunnelConfig,
    child: Arc<Mutex<Option<Child>>>,
}

impl SshTunnel {
    /// 启动 ssh -L 或 -R 子进程。
    ///
    /// 优先使用 ssh-agent 认证，不提供密码交互通道。
    /// 若 ssh 命令不存在或端口被占用，立即返回错误。
    fn start(name: &str, config: &SshTunnelConfig) -> Result<Self, String> {
        let addr = match &config.user {
            Some(user) => format!("{}@{}", user, config.host),
            None => config.host.clone(),
        };

        let forward_flag = match config.direction {
            TunnelDirection::Local => "-L",
            TunnelDirection::Remote => "-R",
        };
        let forward_spec = build_forward_spec(config);

        let child = Command::new("ssh")
            .args([
                forward_flag,
                &forward_spec,
                &addr,
                "-N", // 不执行远程命令
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "PasswordAuthentication=no",       // 优先 ssh-agent
                "-o",
                "StrictHostKeyChecking=accept-new", // 自动接受新主机密钥
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("启动 SSH 隧道 '{name}' 失败: {e}"))?;

        info!(
            "SSH 隧道 '{}' 已启动: {} {} → {} (bind={})",
            name,
            forward_flag,
            forward_spec,
            addr,
            config.bind_address.as_deref().unwrap_or("127.0.0.1"),
        );

        Ok(Self {
            name: name.to_string(),
            _config: config.clone(),
            child: Arc::new(Mutex::new(Some(child))),
        })
    }

    /// 停止隧道（发送 SIGTERM 并等待退出）。
    async fn stop(&self) {
        let mut guard = self.child.lock().await;
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
            info!("SSH 隧道 '{}' 已停止", self.name);
        }
    }
}

/// 管理所有 SSH 隧道的生命周期。
///
/// 在服务器启动时建立隧道，在优雅关闭时回收所有子进程。
pub struct SshTunnelManager {
    tunnels: Vec<SshTunnel>,
}

impl SshTunnelManager {
    /// 根据配置创建并启动所有被引用的隧道。
    ///
    /// 每个隧道对应一个 `ssh -L` 或 `ssh -R` 子进程。
    /// 如果任何一个隧道启动失败，已启动的隧道会被立即关闭。
    pub fn start(configs: &[(String, SshTunnelConfig)]) -> Result<Self, String> {
        let mut tunnels = Vec::with_capacity(configs.len());
        for (name, cfg) in configs {
            let tunnel = SshTunnel::start(name, cfg)?;
            tunnels.push(tunnel);
        }
        Ok(Self { tunnels })
    }

    /// 优雅关闭所有隧道，等待子进程退出。
    pub async fn shutdown(&self) {
        for tunnel in &self.tunnels {
            tunnel.stop().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::config::TunnelDirection;

    fn make_config(
        local_port: u16,
        remote_port: u16,
        direction: TunnelDirection,
        bind_address: Option<&str>,
    ) -> SshTunnelConfig {
        SshTunnelConfig {
            host: "test".to_string(),
            user: None,
            local_port,
            remote_port,
            direction,
            bind_address: bind_address.map(String::from),
        }
    }

    #[test]
    fn test_local_forward_no_bind() {
        let cfg = make_config(8080, 3306, TunnelDirection::Local, None);
        assert_eq!(build_forward_spec(&cfg), "8080:127.0.0.1:3306");
    }

    #[test]
    fn test_local_forward_with_bind() {
        let cfg = make_config(8080, 3306, TunnelDirection::Local, Some("0.0.0.0"));
        assert_eq!(build_forward_spec(&cfg), "0.0.0.0:8080:127.0.0.1:3306");
    }

    #[test]
    fn test_remote_forward_no_bind() {
        let cfg = make_config(8080, 9090, TunnelDirection::Remote, None);
        assert_eq!(build_forward_spec(&cfg), "9090:127.0.0.1:8080");
    }

    #[test]
    fn test_remote_forward_with_bind() {
        let cfg = make_config(8080, 9090, TunnelDirection::Remote, Some("0.0.0.0"));
        assert_eq!(build_forward_spec(&cfg), "0.0.0.0:9090:127.0.0.1:8080");
    }
}
