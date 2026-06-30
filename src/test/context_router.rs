use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::TcpStream;
use tokio::time::{Duration, sleep};

use crate::app::context_routing::ContextRoutingService;
use crate::domain::llm::EmbeddingProvider;
use crate::domain::semantic_classification::{SemanticClassifierT, SemanticInput};
use crate::infra::llm::ollama_embedding_provider::OllamaEmbeddingProvider;
use crate::infra::semantic_classification::EmbeddingSemanticClassifier;
use crate::infra::ssh_tunnel::SshTunnelManager;
use crate::shared::config::AppConfig;

fn real_config_path() -> PathBuf {
    PathBuf::from("config.toml")
}

#[tokio::test]
#[ignore = "需要本机真实 config.toml 和可访问的 embedding 服务；手动运行: cargo test context_router --lib -- --ignored --nocapture"]
async fn context_router() {
    let config_path = real_config_path();
    assert!(
        config_path.exists(),
        "真实配置文件不存在: {}",
        config_path.display()
    );

    let config = AppConfig::load();
    assert!(
        config.semantic_classification.enabled,
        "config.toml 需要配置 semantic_classification.enabled = true"
    );
    assert!(
        config.context_routing.enabled,
        "config.toml 需要配置 context_routing.enabled = true"
    );
    assert!(
        config
            .semantic_classification
            .taxonomy(&config.context_routing.taxonomy)
            .is_some(),
        "config.toml 的 context_routing.taxonomy 必须引用 semantic_classification.taxonomies 中存在的 id"
    );

    let tunnel_manager = ensure_embedding_tunnel(&config).await;

    let embedding_provider: Arc<dyn EmbeddingProvider> =
        Arc::new(OllamaEmbeddingProvider::with_options(
            config.embedding.base_url.clone(),
            config.embedding.model.clone(),
            config.embedding.dimension,
            config.embedding.batch_size,
            config.embedding.timeout_secs,
        ));

    let classifier: Arc<dyn SemanticClassifierT> = Arc::new(
        EmbeddingSemanticClassifier::from_config(
            &config.semantic_classification,
            Arc::clone(&embedding_provider),
        )
        .await
        .expect("真实配置下语义分类器初始化失败"),
    );
    let router = ContextRoutingService::new(classifier, config.context_routing.clone());

    let latest_decision = route_with_real_config(&router, &config, "今天有什么最新 AI 新闻").await;
    assert!(
        latest_decision.fresh_context.enabled,
        "实时/最新问题应启用 Fresh Context，实际决策: {:?}",
        latest_decision
    );
    println!("最新问题决策: {:#?}", latest_decision);

    let memory_decision = route_with_real_config(&router, &config, "记得上次吗").await;
    assert!(
        memory_decision.memory.top_k > 0,
        "个人偏好/记忆问题应保留 Memory 预算，实际决策: {:?}",
        memory_decision
    );
    println!("个人偏好/记忆问题决策: {:#?}", memory_decision);
    assert_eq!(memory_decision.memory.reason, "memory_positive");

    let current_task_decision =
        route_with_real_config(&router, &config, "继续按刚才方案实现").await;
    assert_eq!(
        current_task_decision.rag.top_k, 0,
        "继续当前任务的问题不应拉取 RAG，实际决策: {:?}",
        current_task_decision
    );

    assert_eq!(
        latest_decision.diagnostics.taxonomy,
        config.context_routing.taxonomy
    );
    println!("继续当前任务决策: {:#?}", current_task_decision);

    if let Some(manager) = tunnel_manager {
        manager.shutdown().await;
    }
}

async fn route_with_real_config(
    router: &ContextRoutingService,
    config: &AppConfig,
    text: &str,
) -> crate::app::context_routing::ContextRouteDecision {
    router
        .route(
            SemanticInput::new(text),
            config.agent.max_memory_items,
            u64::from(config.agent.max_rag_chunks),
        )
        .await
}

async fn ensure_embedding_tunnel(config: &AppConfig) -> Option<SshTunnelManager> {
    let tunnel_name = config
        .embedding
        .tunnel
        .as_deref()
        .expect("真实 embedding 测试需要配置 embedding.tunnel");

    let tunnel = config
        .ssh_tunnels
        .get(tunnel_name)
        .unwrap_or_else(|| panic!("未找到 embedding tunnel 配置: ssh_tunnels.{tunnel_name}"))
        .clone();

    if is_local_port_open(tunnel.local_port).await {
        return None;
    }

    let manager = SshTunnelManager::start(&[(tunnel_name.to_string(), tunnel.clone())])
        .unwrap_or_else(|error| panic!("启动 embedding tunnel 失败: {error}"));
    wait_for_local_port(tunnel.local_port).await;
    Some(manager)
}

async fn wait_for_local_port(port: u16) {
    for _ in 0..40 {
        if is_local_port_open(port).await {
            return;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("embedding tunnel 未在本地端口 {port} 就绪");
}

async fn is_local_port_open(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).await.is_ok()
}
