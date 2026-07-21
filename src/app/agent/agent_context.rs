use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, warn};

use crate::app::context_routing::{
    ContextRouteDecision, ContextRoutingService, build_routing_input,
};
use crate::app::fresh_context::retrieval::FreshRetrievalService;
use crate::app::memory::memory_service::MemoryService;
use crate::app::rag::retrieval_service::RetrievalService;
use crate::app::summary::summary_service::SummaryService;
use crate::domain::agent::{AgentContext, ToolDefinition};
use crate::domain::conversation::conversation_repo::ConversationRepoT;
use crate::domain::llm::ChatMessage;
use crate::domain::user::user_profile::UserProfile;
use crate::domain::user::user_profile_repo::UserProfileRepoT;

/// Builder that assembles an `AgentContext` for a single turn.
pub struct AgentContextBuilder {
    memory_service: Arc<MemoryService>,
    retrieval_service: Arc<RetrievalService>,
    summary_service: Arc<SummaryService>,
    fresh_retrieval_service: Option<Arc<FreshRetrievalService>>,
    context_routing_service: Option<Arc<ContextRoutingService>>,
    #[allow(dead_code)]
    conversation_repo: Arc<dyn ConversationRepoT>,
    user_profile_repo: Arc<dyn UserProfileRepoT>,
}

/// Returns the content of the most recent user message from the slice,
/// or an empty string if there are no user messages.
pub fn latest_user_query(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or("")
        .to_string()
}

/// Build a memory recall query from the last N user messages.
/// Using multiple messages provides richer context than the last message alone.
fn build_recall_query(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .filter(|m| m.role == "user")
        .take(3)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

impl AgentContextBuilder {
    pub fn new(
        memory_service: Arc<MemoryService>,
        retrieval_service: Arc<RetrievalService>,
        summary_service: Arc<SummaryService>,
        fresh_retrieval_service: Option<Arc<FreshRetrievalService>>,
        conversation_repo: Arc<dyn ConversationRepoT>,
        user_profile_repo: Arc<dyn UserProfileRepoT>,
    ) -> Self {
        Self {
            memory_service,
            retrieval_service,
            summary_service,
            fresh_retrieval_service,
            context_routing_service: None,
            conversation_repo,
            user_profile_repo,
        }
    }

    pub fn with_context_routing_service(
        mut self,
        context_routing_service: Option<Arc<ContextRoutingService>>,
    ) -> Self {
        self.context_routing_service = context_routing_service;
        self
    }

    pub async fn build(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        recent_messages: Vec<ChatMessage>,
        user_profile: Option<UserProfile>,
        tools: Vec<ToolDefinition>,
        location: Option<Value>,
        max_memory_items: u32,
        max_rag_chunks: u64,
        summary_enabled: bool,
        memory_enabled: bool,
        rag_enabled: bool,
    ) -> AgentContext {
        let summary = if summary_enabled {
            if let Some(cid) = conversation_id {
                self.summary_service
                    .latest_for_conversation(cid)
                    .await
                    .unwrap_or(None)
                    .map(|s| s.content)
            } else {
                None
            }
        } else {
            None
        };

        let recall_query = build_recall_query(&recent_messages);
        let routing_decision = if let Some(context_routing_service) = &self.context_routing_service
        {
            let input = build_routing_input(&recent_messages);
            Some(
                context_routing_service
                    .route(input, max_memory_items, max_rag_chunks)
                    .await,
            )
        } else {
            None
        };
        log_context_route_decision(
            user_id,
            conversation_id,
            routing_decision.as_ref(),
            max_memory_items,
            max_rag_chunks,
        );
        let memory_top_k = routed_memory_top_k(routing_decision.as_ref(), max_memory_items);

        let memories = if !memory_enabled || memory_top_k == 0 {
            Vec::new()
        } else {
            self.memory_service
                .recall(user_id, &recall_query, memory_top_k)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|m| {
                    format!(
                        "[{}] {} (confidence: {:.2})",
                        m.memory_type, m.content, m.confidence
                    )
                })
                .collect()
        };

        let rag_query = latest_user_query(&recent_messages);
        let rag_top_k = routed_rag_top_k(routing_decision.as_ref(), max_rag_chunks);
        let rag_chunks = if !rag_enabled || rag_query.is_empty() || rag_top_k == 0 {
            Vec::new()
        } else {
            self.retrieval_service
                .retrieve(&rag_query, user_id, rag_top_k)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(chunk, _score)| chunk.content)
                .collect()
        };

        let fresh_chunks = if rag_query.is_empty() {
            Vec::new()
        } else if let Some(fresh_retrieval_service) = &self.fresh_retrieval_service {
            let result = match routing_decision.as_ref() {
                Some(decision) if decision.fresh_context.enabled => {
                    fresh_retrieval_service
                        .retrieve_for_routed_query(&rag_query)
                        .await
                }
                Some(_) => Ok(Vec::new()),
                None => fresh_retrieval_service.retrieve_for_query(&rag_query).await,
            };
            match result {
                Ok(contexts) => contexts
                    .into_iter()
                    .map(|context| context.content)
                    .collect(),
                Err(error) => {
                    warn!(error = %error, "Fresh Context 检索失败，继续使用无实时上下文结果");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        log_context_retrieval_counts(
            user_id,
            conversation_id,
            routing_decision.as_ref(),
            memory_top_k,
            rag_top_k,
            memories.len(),
            rag_chunks.len(),
            fresh_chunks.len(),
        );

        let profile: Option<Value> = match user_profile {
            Some(p) => serde_json::to_value(p).ok(),
            None => self
                .user_profile_repo
                .find_by_user_id(user_id)
                .await
                .ok()
                .flatten()
                .and_then(|p| serde_json::to_value(p).ok()),
        };

        AgentContext {
            user_id,
            conversation_id,
            recent_messages,
            summary,
            memories,
            rag_chunks,
            fresh_chunks,
            user_profile: profile,
            tools,
            location,
        }
    }
}

fn routed_memory_top_k(decision: Option<&ContextRouteDecision>, default_top_k: u32) -> u32 {
    decision
        .map(|decision| decision.memory.top_k)
        .unwrap_or(default_top_k)
}

fn routed_rag_top_k(decision: Option<&ContextRouteDecision>, default_top_k: u64) -> u64 {
    decision
        .map(|decision| u64::from(decision.rag.top_k))
        .unwrap_or(default_top_k)
}

fn log_context_route_decision(
    user_id: u64,
    conversation_id: Option<u64>,
    decision: Option<&ContextRouteDecision>,
    default_memory_top_k: u32,
    default_rag_top_k: u64,
) {
    match decision {
        Some(decision) => {
            debug!(
                user_id,
                ?conversation_id,
                routing_enabled = true,
                taxonomy = %decision.diagnostics.taxonomy,
                fresh_enabled = decision.fresh_context.enabled,
                fresh_confidence = decision.fresh_context.confidence,
                memory_top_k = decision.memory.top_k,
                memory_reason = %decision.memory.reason,
                memory_confidence = decision.memory.confidence,
                rag_top_k = decision.rag.top_k,
                rag_reason = %decision.rag.reason,
                rag_confidence = decision.rag.confidence,
                fallback_used = decision.diagnostics.fallback_used,
                top_labels = ?decision.diagnostics.top_labels,
                "Agent 上下文路由决策"
            );
        }
        None => {
            debug!(
                user_id,
                ?conversation_id,
                routing_enabled = false,
                memory_top_k = default_memory_top_k,
                rag_top_k = default_rag_top_k,
                "Agent 上下文路由未启用，使用默认召回预算"
            );
        }
    }
}

fn log_context_retrieval_counts(
    user_id: u64,
    conversation_id: Option<u64>,
    decision: Option<&ContextRouteDecision>,
    memory_top_k: u32,
    rag_top_k: u64,
    memories_count: usize,
    rag_chunks_count: usize,
    fresh_chunks_count: usize,
) {
    debug!(
        user_id,
        ?conversation_id,
        routing_enabled = decision.is_some(),
        fresh_enabled = decision
            .map(|decision| decision.fresh_context.enabled)
            .unwrap_or(false),
        memory_top_k,
        rag_top_k,
        memories_count,
        rag_chunks_count,
        fresh_chunks_count,
        "Agent 上下文召回完成"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::context_routing::{
        ContextRouteDecision, ContextRouteDiagnostics, FreshContextRoute, RetrievalBudgetRoute,
    };

    #[test]
    fn latest_user_query_returns_most_recent() {
        let messages = vec![
            ChatMessage {
                role: "user".into(),
                content: "第一轮问题".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: "回答".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "user".into(),
                content: "第二轮问题".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];
        assert_eq!(latest_user_query(&messages), "第二轮问题");
    }

    #[test]
    fn latest_user_query_still_drives_rag_and_fresh_query() {
        let messages = vec![
            ChatMessage {
                role: "user".into(),
                content: "旧问题".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "user".into(),
                content: "最新问题".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];

        assert_eq!(latest_user_query(&messages), "最新问题");
    }

    #[test]
    fn route_decision_overrides_memory_and_rag_budgets() {
        let decision = ContextRouteDecision {
            fresh_context: FreshContextRoute {
                enabled: false,
                confidence: 0.0,
            },
            memory: RetrievalBudgetRoute {
                top_k: 0,
                confidence: 0.9,
                reason: "memory_negative".into(),
            },
            rag: RetrievalBudgetRoute {
                top_k: 1,
                confidence: 0.5,
                reason: "rag_low_confidence".into(),
            },
            diagnostics: ContextRouteDiagnostics {
                taxonomy: "context_routing".into(),
                top_labels: Vec::new(),
                fallback_used: false,
            },
        };

        assert_eq!(routed_memory_top_k(Some(&decision), 10), 0);
        assert_eq!(routed_rag_top_k(Some(&decision), 5), 1);
        assert_eq!(routed_memory_top_k(None, 10), 10);
        assert_eq!(routed_rag_top_k(None, 5), 5);
    }

    #[test]
    fn route_decision_debug_helpers_accept_disabled_and_enabled_paths() {
        let decision = ContextRouteDecision {
            fresh_context: FreshContextRoute {
                enabled: true,
                confidence: 0.91,
            },
            memory: RetrievalBudgetRoute {
                top_k: 7,
                confidence: 0.82,
                reason: "memory_positive".into(),
            },
            rag: RetrievalBudgetRoute {
                top_k: 2,
                confidence: 0.66,
                reason: "rag_low_confidence".into(),
            },
            diagnostics: ContextRouteDiagnostics {
                taxonomy: "context_routing".into(),
                top_labels: vec![
                    ("context.memory.positive".into(), 0.82),
                    ("context.rag.positive".into(), 0.66),
                ],
                fallback_used: false,
            },
        };

        log_context_route_decision(42, Some(9), Some(&decision), 10, 5);
        log_context_retrieval_counts(42, Some(9), Some(&decision), 7, 2, 3, 2, 1);
        log_context_route_decision(42, None, None, 10, 5);
        log_context_retrieval_counts(42, None, None, 10, 5, 0, 0, 0);
    }

    #[test]
    fn latest_user_query_empty_on_no_user() {
        let messages = vec![ChatMessage {
            role: "assistant".into(),
            content: "only assistant".into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        assert_eq!(latest_user_query(&messages), "");
    }

    #[test]
    fn latest_user_query_empty_on_empty_slice() {
        let messages: Vec<ChatMessage> = vec![];
        assert_eq!(latest_user_query(&messages), "");
    }

    #[test]
    fn build_recall_query_uses_multiple_user_messages() {
        let msgs = vec![
            ChatMessage {
                role: "user".into(),
                content: "first question".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: "first answer".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "user".into(),
                content: "second question".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];
        let query = build_recall_query(&msgs);
        assert!(query.contains("first question"));
        assert!(query.contains("second question"));
    }

    #[test]
    fn build_recall_query_empty_on_no_user_messages() {
        let msgs = vec![ChatMessage {
            role: "assistant".into(),
            content: "only assistant".into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        let query = build_recall_query(&msgs);
        assert_eq!(query, "");
    }

    #[test]
    fn build_recall_query_empty_on_empty_slice() {
        let msgs: Vec<ChatMessage> = vec![];
        let query = build_recall_query(&msgs);
        assert_eq!(query, "");
    }
}
