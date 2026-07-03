use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::domain::llm::LlmProvider;
use crate::infra::llm::ollama_provider::OllamaProvider;
use crate::infra::repo::connection::init_db;
use crate::infra::ssh_tunnel::SshTunnelManager;
use crate::shared::config::AppConfig;

/// SSH 隧道、数据库连接、LLM Provider。
pub struct InfraContext {
    pub db: DatabaseConnection,
    pub ollama_provider: Arc<dyn LlmProvider>,
    pub _ssh_manager: Option<SshTunnelManager>,
}

impl InfraContext {
    /// 建立 SSH 隧道 → 数据库连接 → LLM Provider。
    pub async fn new(config: &AppConfig) -> Result<Self, std::io::Error> {
        let _ssh_manager = start_ssh_tunnels(config)?;

        let db = init_db(&config.database.url, config.database.max_connections)
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let ollama_provider: Arc<dyn LlmProvider> = Arc::new(OllamaProvider::with_timeout(
            config.llm.base_url.clone(),
            config.llm.chat_model.clone(),
            config.llm.timeout_secs,
        ));

        Ok(Self {
            db,
            ollama_provider,
            _ssh_manager,
        })
    }
}

// ── SSH 隧道（从 main.rs 移入）──

/// 启动 SSH 隧道管理器。
///
/// - `-R`（远程转发）隧道无条件启动，用于暴露端口到公网。
/// - `-L`（本地转发）隧道仅在被业务配置引用时启动。
fn start_ssh_tunnels(config: &AppConfig) -> Result<Option<SshTunnelManager>, std::io::Error> {
    if config.ssh_tunnels.is_empty() {
        return Ok(None);
    }

    let used_tunnels = config.active_ssh_tunnels();

    if used_tunnels.is_empty() {
        return Ok(None);
    }

    tracing::info!(
        "正在启动 SSH 隧道: {}",
        used_tunnels
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let manager = SshTunnelManager::start(&used_tunnels)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    Ok(Some(manager))
}
