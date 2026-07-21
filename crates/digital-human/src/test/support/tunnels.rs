use std::collections::BTreeMap;

use tokio::net::TcpStream;
use tokio::time::{Duration, sleep};

use crate::infra::ssh_tunnel::SshTunnelManager;
use crate::shared::config::{AppConfig, SshTunnelConfig, TunnelDirection};

#[derive(Clone, Copy)]
pub enum ServiceTunnel {
    Database,
    VectorStore,
    Embedding,
    Llm,
}

#[derive(Clone, Copy)]
pub enum TunnelRequirement {
    Optional(ServiceTunnel),
    Required(ServiceTunnel),
}

pub async fn ensure(
    config: &AppConfig,
    requirements: &[TunnelRequirement],
    scenario: &str,
) -> Option<SshTunnelManager> {
    let mut required = BTreeMap::<String, SshTunnelConfig>::new();

    for requirement in requirements {
        let (service, required_flag) = match requirement {
            TunnelRequirement::Optional(service) => (*service, false),
            TunnelRequirement::Required(service) => (*service, true),
        };
        insert_tunnel(&mut required, config, service, required_flag, scenario);
    }

    let mut to_start = Vec::new();
    for (name, tunnel) in required {
        if !matches!(tunnel.direction, TunnelDirection::Local) {
            panic!("{scenario} 只会自动启动本地转发隧道，{name} 不是 local 隧道");
        }
        if !is_local_port_open(tunnel.local_port).await {
            to_start.push((name, tunnel));
        }
    }

    if to_start.is_empty() {
        return None;
    }

    let manager = SshTunnelManager::start(&to_start)
        .unwrap_or_else(|error| panic!("启动 {scenario} 所需 SSH 隧道失败: {error}"));
    for (_, tunnel) in &to_start {
        wait_for_local_port(tunnel.local_port, scenario).await;
    }
    Some(manager)
}

fn insert_tunnel(
    required: &mut BTreeMap<String, SshTunnelConfig>,
    config: &AppConfig,
    service: ServiceTunnel,
    required_flag: bool,
    scenario: &str,
) {
    let Some(name) = tunnel_name(config, service) else {
        if required_flag {
            panic!("{scenario} 需要配置 {}.tunnel", service.config_key());
        }
        return;
    };

    let tunnel = config
        .ssh_tunnels
        .get(name)
        .unwrap_or_else(|| panic!("未找到 SSH 隧道配置: ssh_tunnels.{name}"))
        .clone();
    required.insert(name.to_string(), tunnel);
}

fn tunnel_name(config: &AppConfig, service: ServiceTunnel) -> Option<&str> {
    match service {
        ServiceTunnel::Database => config.database.tunnel.as_deref(),
        ServiceTunnel::VectorStore => config.vector_store.tunnel.as_deref(),
        ServiceTunnel::Embedding => config.embedding.tunnel.as_deref(),
        ServiceTunnel::Llm => config.llm.tunnel.as_deref(),
    }
    .map(str::trim)
    .filter(|name| !name.is_empty())
}

impl ServiceTunnel {
    fn config_key(self) -> &'static str {
        match self {
            ServiceTunnel::Database => "database",
            ServiceTunnel::VectorStore => "vector_store",
            ServiceTunnel::Embedding => "embedding",
            ServiceTunnel::Llm => "llm",
        }
    }
}

async fn wait_for_local_port(port: u16, scenario: &str) {
    for _ in 0..40 {
        if is_local_port_open(port).await {
            return;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("{scenario} 所需 SSH 隧道未在本地端口 {port} 就绪");
}

async fn is_local_port_open(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).await.is_ok()
}
