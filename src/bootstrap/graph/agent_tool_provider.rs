use std::sync::Arc;

use crate::app::agent::agent_runtime::AgentTool;
use crate::app::agent::tool_registry::{AgentToolDeps, build_default_agent_tools};
use crate::app::memory::memory_service::MemoryService;
use crate::app::rag::retrieval_service::RetrievalService;
use crate::domain::http::HttpClientT;
use crate::infra::http::{RedirectPolicy, ReqwestHttpClient, ReqwestHttpClientConfig};

use super::BootstrapContext;

pub(crate) fn build_agent_tools(
    ctx: &BootstrapContext<'_>,
    retrieval: Arc<RetrievalService>,
    memory: Arc<MemoryService>,
) -> Result<Vec<Arc<dyn AgentTool>>, std::io::Error> {
    let tool_deps = AgentToolDeps {
        retrieval,
        memory,
        diary_repo: Arc::clone(&ctx.repos.diary_repo),
        depression_repo: Arc::clone(&ctx.repos.depression_repo),
        music_repo: Arc::clone(&ctx.repos.music_repo),
        community_repo: Arc::clone(&ctx.repos.community_repo),
        plugins: ctx.config.plugins.clone(),
        fetch_web_content_http_client: build_http_client(ReqwestHttpClientConfig {
            connect_timeout_secs: 10,
            timeout_secs: 20,
            redirect_policy: RedirectPolicy::None,
            proxy_url: Some(ctx.config.plugins.fetch_web_content.proxy_url.clone()),
            no_proxy: true,
        })?,
        baidu_baike_http_client: build_http_client(ReqwestHttpClientConfig {
            connect_timeout_secs: 10,
            timeout_secs: 15,
            redirect_policy: RedirectPolicy::Limited(5),
            proxy_url: Some(ctx.config.plugins.baidu_baike.proxy_url.clone()),
            no_proxy: true,
        })?,
        weather_http_client: build_http_client(ReqwestHttpClientConfig {
            connect_timeout_secs: 10,
            timeout_secs: 20,
            redirect_policy: RedirectPolicy::Limited(5),
            proxy_url: None,
            no_proxy: false,
        })?,
    };

    build_default_agent_tools(&tool_deps, ctx.config.agent.enabled)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error.to_string()))
}

fn build_http_client(
    config: ReqwestHttpClientConfig,
) -> Result<Arc<dyn HttpClientT>, std::io::Error> {
    let client = ReqwestHttpClient::new(config)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    Ok(Arc::new(client))
}
