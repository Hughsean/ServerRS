use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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

/// 本地转发必须独占监听端口。若端口已被其他服务占用，继续启动会让业务误连该服务，
/// 因此在创建 SSH 子进程前先 fail closed。
fn ensure_local_forward_port_available(config: &SshTunnelConfig) -> Result<(), String> {
    if !matches!(config.direction, TunnelDirection::Local) {
        return Ok(());
    }

    let bind_address = config.bind_address.as_deref().unwrap_or("127.0.0.1");
    TcpListener::bind((bind_address, config.local_port))
        .map(drop)
        .map_err(|error| {
            format!(
                "SSH 本地转发端口 {bind_address}:{} 不可用，拒绝启动以避免连接错误服务: {error}",
                config.local_port
            )
        })
}

const SSH_TUNNEL_START_TIMEOUT: Duration = Duration::from_secs(10);
const SSH_REMOTE_TUNNEL_OBSERVATION: Duration = Duration::from_millis(250);
const SSH_TUNNEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 等待 SSH 真正建立本地监听，避免后续依赖在隧道尚未就绪时立即发起连接。
fn wait_until_tunnel_ready(
    name: &str,
    config: &SshTunnelConfig,
    child: &mut Child,
) -> Result<(), String> {
    let probe_addr = local_forward_probe_addr(config);
    let timeout = if probe_addr.is_some() {
        SSH_TUNNEL_START_TIMEOUT
    } else {
        SSH_REMOTE_TUNNEL_OBSERVATION
    };
    let deadline = Instant::now() + timeout;

    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("检查 SSH 隧道 '{name}' 进程失败: {error}"))?
        {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            let detail = stderr.trim();
            return Err(if detail.is_empty() {
                format!("SSH 隧道 '{name}' 在就绪前退出: {status}")
            } else {
                format!("SSH 隧道 '{name}' 在就绪前退出: {status}: {detail}")
            });
        }

        match probe_addr {
            Some(addr) if TcpStream::connect_timeout(&addr, SSH_TUNNEL_POLL_INTERVAL).is_ok() => {
                return Ok(());
            }
            // 远程转发没有本地监听端口；短暂观察进程，捕获常见的即时启动失败。
            None if Instant::now() + SSH_TUNNEL_POLL_INTERVAL >= deadline => return Ok(()),
            _ if Instant::now() >= deadline => {
                return Err(format!(
                    "等待 SSH 隧道 '{name}' 本地端口 {} 就绪超时",
                    config.local_port
                ));
            }
            _ => thread::sleep(SSH_TUNNEL_POLL_INTERVAL),
        }
    }
}

fn local_forward_probe_addr(config: &SshTunnelConfig) -> Option<SocketAddr> {
    if !matches!(config.direction, TunnelDirection::Local) {
        return None;
    }

    let ip = config
        .bind_address
        .as_deref()
        .and_then(|address| address.parse::<IpAddr>().ok())
        .map(|address| match address {
            IpAddr::V4(address) if address.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(address) if address.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            address => address,
        })
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));

    Some(SocketAddr::new(ip, config.local_port))
}

/// 单个 SSH 隧道实例，持有 ssh 子进程句柄。
struct SshTunnel {
    name: String,
    _config: SshTunnelConfig,
    child: Arc<Mutex<Option<Child>>>,
    /// Windows Job Object 在父进程被强制终止时自动杀死 ssh 子进程树。
    #[cfg(windows)]
    _job: Option<WindowsJob>,
}

#[cfg(windows)]
struct WindowsJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl WindowsJob {
    fn new() -> Result<Self, String> {
        use std::mem::{size_of, zeroed};
        use std::ptr::null;
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        // SAFETY: 使用无名称、默认安全属性的 Job Object；返回的句柄仅由 Self 持有。
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(format!(
                "创建 SSH Job Object 失败: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: limits 指针和长度与信息类别完全匹配，handle 是刚创建的有效 Job Object。
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            // SAFETY: 当前路径仍独占这个有效句柄。
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
            return Err(format!(
                "配置 SSH Job Object 失败: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self(handle))
    }

    fn assign(&self, child: &Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        // SAFETY: child 仍由调用方持有且未退出；Job Object 句柄由 Self 持有。
        let assigned = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) };
        if assigned == 0 {
            return Err(format!(
                "将 SSH 子进程加入 Job Object 失败: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        // SAFETY: Self 独占句柄；KILL_ON_JOB_CLOSE 由内核终止仍在 Job 中的进程树。
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

impl SshTunnel {
    /// 启动 ssh -L 或 -R 子进程。
    ///
    /// 优先使用 ssh-agent 认证，不提供密码交互通道。
    /// 若 ssh 命令不存在或端口被占用，立即返回错误。
    fn start(name: &str, config: &SshTunnelConfig) -> Result<Self, String> {
        ensure_local_forward_port_available(config)?;

        let addr = match &config.user {
            Some(user) => format!("{}@{}", user, config.host),
            None => config.host.clone(),
        };

        let forward_flag = match config.direction {
            TunnelDirection::Local => "-L",
            TunnelDirection::Remote => "-R",
        };
        let forward_spec = build_forward_spec(config);

        let mut child = Command::new("ssh")
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
                "PasswordAuthentication=no", // 优先 ssh-agent
                "-o",
                "StrictHostKeyChecking=accept-new", // 自动接受新主机密钥
                "ExitOnForwardFailure=yes",         // 转发失败时退出
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("启动 SSH 隧道 '{name}' 失败: {e}"))?;

        #[cfg(windows)]
        let job = match WindowsJob::new().and_then(|job| {
            job.assign(&child)?;
            Ok(job)
        }) {
            Ok(job) => Some(job),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "启动 SSH 隧道 '{name}' 时无法纳入 Windows Job Object: {error}"
                ));
            }
        };

        if let Err(error) = wait_until_tunnel_ready(name, config, &mut child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }

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
            #[cfg(windows)]
            _job: job,
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

    fn stop_on_drop(&self) {
        let Ok(mut guard) = self.child.try_lock() else {
            return;
        };

        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
            info!("SSH 隧道 '{}' 已停止", self.name);
        }
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        self.stop_on_drop();
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

    #[test]
    fn local_forward_rejects_an_occupied_port() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind test listener");
        let port = listener.local_addr().expect("test listener address").port();
        let cfg = make_config(port, 3306, TunnelDirection::Local, None);

        let error = ensure_local_forward_port_available(&cfg)
            .expect_err("occupied local-forward port must be rejected");

        assert!(error.contains(&port.to_string()));
        assert!(error.contains("拒绝启动"));
    }

    #[test]
    fn remote_forward_does_not_claim_the_local_service_port() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind test listener");
        let port = listener.local_addr().expect("test listener address").port();
        let cfg = make_config(port, 8080, TunnelDirection::Remote, None);

        ensure_local_forward_port_available(&cfg)
            .expect("remote forwarding must not preflight the local service port");
    }

    #[test]
    fn local_forward_waits_until_the_port_is_listening() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind test listener");
        let port = listener.local_addr().expect("test listener address").port();
        let config = make_config(port, 6334, TunnelDirection::Local, None);
        let mut child = spawn_sleep_child();

        let result = wait_until_tunnel_ready("test", &config, &mut child);
        let _ = child.kill();
        let _ = child.wait();

        assert!(result.is_ok());
    }

    #[test]
    fn dropping_tunnel_kills_child_process() {
        let child = spawn_sleep_child();
        let pid = child.id();
        let tunnel = SshTunnel {
            name: "test".to_string(),
            _config: make_config(8080, 3306, TunnelDirection::Local, None),
            child: Arc::new(Mutex::new(Some(child))),
            #[cfg(windows)]
            _job: None,
        };

        drop(tunnel);
        std::thread::sleep(Duration::from_millis(250));

        let still_running = process_is_running(pid);
        if still_running {
            kill_process(pid);
        }

        assert!(
            !still_running,
            "dropping a tunnel should stop its child process"
        );
    }

    #[cfg(windows)]
    fn spawn_sleep_child() -> Child {
        Command::new("powershell")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 60"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep child")
    }

    #[cfg(unix)]
    fn spawn_sleep_child() -> Child {
        Command::new("sleep")
            .arg("60")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep child")
    }

    #[cfg(windows)]
    fn process_is_running(pid: u32) -> bool {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "if (Get-Process -Id {} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}",
                    pid
                ),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    fn process_is_running(pid: u32) -> bool {
        Command::new("sh")
            .args(["-c", &format!("kill -0 {}", pid)])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    fn kill_process(pid: u32) {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F", "/T"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    #[cfg(unix)]
    fn kill_process(pid: u32) {
        let _ = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
