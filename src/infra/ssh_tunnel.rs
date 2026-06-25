use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::info;

use crate::shared::config::SshTunnelConfig;

/// 单个 SSH 隧道实例，持有 ssh -L 子进程句柄。
struct SshTunnel {
    name: String,
    _config: SshTunnelConfig,
    child: Arc<Mutex<Option<Child>>>,
}

impl SshTunnel {
    /// 启动 ssh -L 子进程。
    ///
    /// 优先使用 ssh-agent 认证，不提供密码交互通道。
    /// 若 ssh 命令不存在或端口被占用，立即返回错误。
    fn start(name: &str, config: &SshTunnelConfig) -> Result<Self, String> {
        let local = format!("{}:localhost:{}", config.local_port, config.remote_port);
        let addr = match &config.user {
            Some(user) => format!("{}@{}", user, config.host),
            None => config.host.clone(),
        };

        let child = Command::new("ssh")
            .args([
                "-L",
                &local,
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
            "SSH 隧道 '{}' 已启动: {} → localhost:{}",
            name, addr, config.remote_port
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
    /// 每个隧道对应一个 `ssh -L` 子进程。
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
