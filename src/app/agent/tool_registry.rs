use std::collections::HashSet;
use std::sync::Arc;

use tracing::info;

use crate::app::agent::agent_runtime::AgentTool;
use crate::app::agent::tools::baidu_baike_tool::BaiduBaikeTool;
use crate::app::agent::tools::community_search_tool::CommunitySearchTool;
use crate::app::agent::tools::depression_scale_tool::DepressionScaleTool;
use crate::app::agent::tools::diary_search_tool::DiarySearchTool;
use crate::app::agent::tools::fetch_web_content_tool::FetchWebContentTool;
use crate::app::agent::tools::get_time_tool::GetTimeTool;
use crate::app::agent::tools::get_weather_tool::GetWeatherTool;
use crate::app::agent::tools::knowledge_search_tool::KnowledgeSearchTool;
use crate::app::agent::tools::memory_search_tool::MemorySearchTool;
use crate::app::agent::tools::music_recommend_tool::MusicRecommendTool;
use crate::app::memory::memory_service::MemoryService;
use crate::app::rag::retrieval_service::RetrievalService;
use crate::domain::community::CommunityRepository;
use crate::domain::depression::DepressionRepository;
use crate::domain::diary::DiaryRepository;
use crate::domain::music::MusicRepository;
use crate::shared::config::PluginsConfig;
use crate::shared::error::AppError;

// ── Agent Tool Dependencies ────────────────────────────────────────────────

#[derive(Clone)]
pub struct AgentToolDeps {
    pub retrieval: Arc<RetrievalService>,
    pub memory: Arc<MemoryService>,
    pub diary_repo: Arc<dyn DiaryRepository>,
    pub depression_repo: Arc<dyn DepressionRepository>,
    pub music_repo: Arc<dyn MusicRepository>,
    pub community_repo: Arc<dyn CommunityRepository>,
    pub plugins: PluginsConfig,
}

// ── Agent Tool Registration ────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct AgentToolRegistration {
    pub key: &'static str,
    pub order: u16,
    pub enabled_by_default: bool,
    pub factory: fn(&AgentToolDeps) -> Arc<dyn AgentTool>,
}

// ── Default Registrations ──────────────────────────────────────────────────

pub fn default_agent_tool_registrations() -> Vec<AgentToolRegistration> {
    vec![
        AgentToolRegistration {
            key: "knowledge_search",
            order: 10,
            enabled_by_default: true,
            factory: |deps| Arc::new(KnowledgeSearchTool::new(Arc::clone(&deps.retrieval))),
        },
        AgentToolRegistration {
            key: "memory_search",
            order: 20,
            enabled_by_default: true,
            factory: |deps| Arc::new(MemorySearchTool::new(Arc::clone(&deps.memory))),
        },
        AgentToolRegistration {
            key: "diary_search",
            order: 30,
            enabled_by_default: true,
            factory: |deps| Arc::new(DiarySearchTool::new(Arc::clone(&deps.diary_repo))),
        },
        AgentToolRegistration {
            key: "depression_scale",
            order: 40,
            enabled_by_default: true,
            factory: |deps| Arc::new(DepressionScaleTool::new(Arc::clone(&deps.depression_repo))),
        },
        AgentToolRegistration {
            key: "music_recommend",
            order: 50,
            enabled_by_default: true,
            factory: |deps| Arc::new(MusicRecommendTool::new(Arc::clone(&deps.music_repo))),
        },
        AgentToolRegistration {
            key: "community_search",
            order: 60,
            enabled_by_default: true,
            factory: |deps| Arc::new(CommunitySearchTool::new(Arc::clone(&deps.community_repo))),
        },
        AgentToolRegistration {
            key: "get_time",
            order: 80,
            enabled_by_default: true,
            factory: |_deps| Arc::new(GetTimeTool::new()),
        },
        AgentToolRegistration {
            key: "fetch_web_content",
            order: 90,
            enabled_by_default: true,
            factory: |deps| {
                Arc::new(FetchWebContentTool::new(
                    deps.plugins.fetch_web_content.clone(),
                ))
            },
        },
        AgentToolRegistration {
            key: "get_baidu_baike",
            order: 100,
            enabled_by_default: true,
            factory: |deps| Arc::new(BaiduBaikeTool::new(deps.plugins.baidu_baike.clone())),
        },
        AgentToolRegistration {
            key: "get_weather",
            order: 120,
            enabled_by_default: true,
            factory: |deps| Arc::new(GetWeatherTool::new(deps.plugins.weather.clone())),
        },
    ]
}

// ── Build ──────────────────────────────────────────────────────────────────

pub fn build_default_agent_tools(
    deps: &AgentToolDeps,
    agent_enabled: bool,
) -> Result<Vec<Arc<dyn AgentTool>>, AppError> {
    if !agent_enabled {
        info!("agent is disabled — returning empty tool list");
        return Ok(Vec::new());
    }

    let mut registrations = default_agent_tool_registrations();

    registrations.sort_by_key(|registration| registration.order);

    validate_registration_keys(&registrations)?;

    let mut tools = Vec::new();

    for registration in registrations
        .into_iter()
        .filter(|registration| registration.enabled_by_default)
    {
        // Conditional filtering based on config
        let should_register = match registration.key {
            "fetch_web_content" => deps.plugins.fetch_web_content.enabled,
            "get_baidu_baike" => deps.plugins.baidu_baike.enabled,
            "get_weather" => !deps.plugins.weather.api_key.trim().is_empty(),
            _ => true,
        };

        if !should_register {
            info!(
                tool = registration.key,
                "tool skipped — disabled or not configured"
            );
            continue;
        }

        let tool = (registration.factory)(deps);
        tools.push(tool);
    }

    validate_tool_names(&tools)?;

    info!(
        tools = ?tools
            .iter()
            .map(|tool| tool.name().to_string())
            .collect::<Vec<_>>(),
        "agent tools registered"
    );

    Ok(tools)
}

// ── Validation ─────────────────────────────────────────────────────────────

fn validate_registration_keys(registrations: &[AgentToolRegistration]) -> Result<(), AppError> {
    let mut keys = HashSet::new();

    for registration in registrations {
        if !keys.insert(registration.key) {
            return Err(AppError::internal(format!(
                "duplicate agent tool registration key: {}",
                registration.key
            )));
        }
    }

    Ok(())
}

fn validate_tool_names(tools: &[Arc<dyn AgentTool>]) -> Result<(), AppError> {
    let mut names = HashSet::new();

    for tool in tools {
        let name = tool.name();

        if name.trim().is_empty() {
            return Err(AppError::internal("agent tool name cannot be empty"));
        }

        if !names.insert(name.to_string()) {
            return Err(AppError::internal(format!(
                "duplicate agent tool name: {}",
                name
            )));
        }
    }

    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registrations_are_sorted_after_build_sort_key_is_applied() {
        let mut registrations = default_agent_tool_registrations();
        registrations.sort_by_key(|registration| registration.order);

        let keys = registrations
            .iter()
            .map(|registration| registration.key)
            .collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec![
                "knowledge_search",
                "memory_search",
                "diary_search",
                "depression_scale",
                "music_recommend",
                "community_search",
                "get_time",
                "fetch_web_content",
                "get_baidu_baike",
                "get_weather",
            ]
        );
    }

    #[test]
    fn default_registration_keys_are_unique() {
        let registrations = default_agent_tool_registrations();
        validate_registration_keys(&registrations).expect("default tool keys must be unique");
    }

    #[test]
    fn default_registrations_do_not_include_risk_tools() {
        assert!(
            default_agent_tool_registrations()
                .iter()
                .all(|registration| !registration.key.contains("risk"))
        );
    }
}
