use std::sync::Arc;

use crate::app::context_routing::ContextRoutingService;
use crate::domain::semantic_classification::SemanticInput;
use crate::shared::config::AppConfig;
use crate::test::support::{config, infra, logging, tunnels};

#[tokio::test]
#[ignore = "需要本机 config.toml 和可访问的 embedding 服务；手动运行: cargo test context_router --lib -- --ignored --nocapture"]
async fn context_router() {
    logging::init();

    let config = config::load();
    config::require_context_routing(&config, "上下文路由测试");

    let tunnel_manager = tunnels::ensure(
        &config,
        &[tunnels::TunnelRequirement::Required(
            tunnels::ServiceTunnel::Embedding,
        )],
        "上下文路由测试",
    )
    .await;

    let embedding_provider = infra::embedding_provider(&config);
    let router = infra::context_routing_service(&config, Arc::clone(&embedding_provider)).await;

    let latest_decision = route_with_config(&router, &config, "今天有什么最新 AI 新闻").await;
    assert!(
        latest_decision.fresh_context.enabled,
        "实时/最新问题应启用 Fresh Context，实际决策: {:?}",
        latest_decision
    );
    println!("最新问题决策: {:#?}", latest_decision);

    let memory_decision = route_with_config(&router, &config, "记得上次吗").await;
    assert!(
        memory_decision.memory.top_k > 0,
        "个人偏好/记忆问题应保留 Memory 预算，实际决策: {:?}",
        memory_decision
    );
    println!("个人偏好/记忆问题决策: {:#?}", memory_decision);
    assert_eq!(memory_decision.memory.reason, "memory_positive");

    let current_task_decision = route_with_config(&router, &config, "继续按刚才方案实现").await;
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

async fn route_with_config(
    router: &Arc<ContextRoutingService>,
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
