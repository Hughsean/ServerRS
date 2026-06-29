use std::path::PathBuf;
use std::sync::Arc;

use crate::app::context_routing::ContextRoutingService;
use crate::domain::llm::EmbeddingProvider;
use crate::domain::semantic_classification::{SemanticClassifierT, SemanticInput};
use crate::infra::llm::ollama_embedding_provider::OllamaEmbeddingProvider;
use crate::infra::semantic_classification::EmbeddingSemanticClassifier;
use crate::shared::config::AppConfig;

fn real_config_path() -> PathBuf {
    std::env::var_os("CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}

#[test]
#[ignore = "需要本机真实 config.toml/.env；手动运行: cargo test real_env --lib -- --ignored"]
fn real_env_config_file_loads_and_validates() {
    let config_path = real_config_path();
    assert!(
        config_path.exists(),
        "真实配置文件不存在: {}",
        config_path.display()
    );

    let config = AppConfig::load();

    config.validate().expect("真实配置校验失败");
    assert!(
        !config.database.url.trim().is_empty(),
        "database.url 不能为空"
    );
    assert!(
        !config.llm.base_url.trim().is_empty(),
        "llm.base_url 不能为空"
    );
    assert!(
        !config.embedding.base_url.trim().is_empty(),
        "embedding.base_url 不能为空"
    );
}

#[tokio::test]
#[ignore = "需要本机真实 config.toml/.env 和可访问的 embedding 服务；手动运行: cargo test real_env --lib -- --ignored"]
async fn real_env_context_router_initializes_when_enabled() {
    let config_path = real_config_path();
    assert!(
        config_path.exists(),
        "真实配置文件不存在: {}",
        config_path.display()
    );

    let config = AppConfig::load();
    if !config.context_routing.enabled {
        println!("真实配置未启用 context_routing，跳过路由初始化测试");
        return;
    }

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

    let decision = router
        .route(
            SemanticInput::new("今天有什么最新消息"),
            config.agent.max_memory_items,
            u64::from(config.agent.max_rag_chunks),
        )
        .await;

    assert_eq!(
        decision.diagnostics.taxonomy,
        config.context_routing.taxonomy
    );
}
