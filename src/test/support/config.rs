use std::path::PathBuf;

use crate::shared::config::AppConfig;

pub fn load() -> AppConfig {
    let path = PathBuf::from("config.toml");
    assert!(path.exists(), "测试配置文件不存在: {}", path.display());
    AppConfig::load()
}

pub fn require_agent(config: &AppConfig, scenario: &str) {
    assert!(
        config.agent.enabled,
        "{scenario} 需要配置 agent.enabled = true"
    );
}

pub fn require_qdrant(config: &AppConfig, scenario: &str) {
    assert!(
        config.qdrant.enabled,
        "{scenario} 需要配置 qdrant.enabled = true"
    );
}

pub fn require_context_routing(config: &AppConfig, scenario: &str) {
    assert!(
        config.context_routing.enabled,
        "{scenario} 需要配置 context_routing.enabled = true"
    );
    assert!(
        config.semantic_classification.enabled,
        "{scenario} 需要配置 semantic_classification.enabled = true"
    );
    assert!(
        config
            .semantic_classification
            .taxonomy(&config.context_routing.taxonomy)
            .is_some(),
        "{scenario} 需要 context_routing.taxonomy 引用 semantic_classification.taxonomies 中存在的 id"
    );
}

pub fn require_memory(config: &AppConfig, scenario: &str) {
    assert!(
        config.agent.memory_enabled,
        "{scenario} 需要配置 agent.memory_enabled = true"
    );
}

pub fn require_tools(config: &AppConfig, scenario: &str) {
    assert!(
        config.llm.max_tool_depth > 0,
        "{scenario} 需要配置 llm.max_tool_depth > 0"
    );
}
