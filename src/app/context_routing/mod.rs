use std::sync::Arc;

use tracing::{debug, warn};

use crate::domain::llm::ChatMessage;
use crate::domain::semantic_classification::{
    SemanticClassificationSet, SemanticClassifierT, SemanticInput, SemanticTaxonomyId,
};
use crate::shared::config::ContextRoutingConfig;
use crate::shared::config::context_routing::{RagRouteConfig, RetrievalRouteConfig};

const FRESH_POSITIVE: &str = "context.fresh.positive";
const FRESH_NEGATIVE: &str = "context.fresh.negative";
const MEMORY_POSITIVE: &str = "context.memory.positive";
const MEMORY_NEGATIVE: &str = "context.memory.negative";
const RAG_POSITIVE: &str = "context.rag.positive";
const RAG_NEGATIVE: &str = "context.rag.negative";

#[derive(Debug, Clone, PartialEq)]
pub struct ContextRouteDecision {
    pub fresh_context: FreshContextRoute,
    pub memory: RetrievalBudgetRoute,
    pub rag: RetrievalBudgetRoute,
    pub diagnostics: ContextRouteDiagnostics,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FreshContextRoute {
    pub enabled: bool,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalBudgetRoute {
    pub top_k: u32,
    pub confidence: f64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextRouteDiagnostics {
    pub taxonomy: String,
    pub top_labels: Vec<(String, f64)>,
    pub fallback_used: bool,
}

pub struct ContextRoutingService {
    classifier: Arc<dyn SemanticClassifierT>,
    config: ContextRoutingConfig,
}

impl ContextRoutingService {
    pub fn new(classifier: Arc<dyn SemanticClassifierT>, config: ContextRoutingConfig) -> Self {
        Self { classifier, config }
    }

    pub async fn route(
        &self,
        input: SemanticInput,
        max_memory_items: u32,
        max_rag_chunks: u64,
    ) -> ContextRouteDecision {
        let taxonomy = SemanticTaxonomyId::new(self.config.taxonomy.clone());
        match self.classifier.classify(&taxonomy, input).await {
            Ok(set) => self.decision_from_set(set, max_memory_items, max_rag_chunks),
            Err(error) => {
                warn!(error = %error, "上下文路由分类器失败，使用保守回退");
                self.fallback_decision(max_memory_items, max_rag_chunks)
            }
        }
    }

    fn decision_from_set(
        &self,
        set: SemanticClassificationSet,
        max_memory_items: u32,
        max_rag_chunks: u64,
    ) -> ContextRouteDecision {
        let fresh_pos = set.score_for(FRESH_POSITIVE);
        let fresh_neg = set.score_for(FRESH_NEGATIVE);
        let fresh_enabled = fresh_pos >= self.config.fresh_context.positive_threshold
            && fresh_neg < self.config.fresh_context.negative_threshold
            && fresh_pos >= fresh_neg + self.config.margin;

        let memory = route_retrieval(
            &self.config.memory,
            set.score_for(MEMORY_POSITIVE),
            set.score_for(MEMORY_NEGATIVE),
            self.config.margin,
            max_memory_items,
            "memory",
        );

        let rag = route_rag(
            &self.config.rag,
            set.score_for(RAG_POSITIVE),
            set.score_for(RAG_NEGATIVE),
            self.config.margin,
            max_rag_chunks,
        );

        let diagnostics = diagnostics_from_set(&set);
        debug!(
            taxonomy = %diagnostics.taxonomy,
            fresh_enabled,
            memory_top_k = memory.top_k,
            rag_top_k = rag.top_k,
            fallback = diagnostics.fallback_used,
            "上下文路由决策"
        );

        ContextRouteDecision {
            fresh_context: FreshContextRoute {
                enabled: fresh_enabled,
                confidence: fresh_pos.max(fresh_neg),
            },
            memory,
            rag,
            diagnostics,
        }
    }

    fn fallback_decision(
        &self,
        max_memory_items: u32,
        max_rag_chunks: u64,
    ) -> ContextRouteDecision {
        ContextRouteDecision {
            fresh_context: FreshContextRoute {
                enabled: false,
                confidence: 0.0,
            },
            memory: RetrievalBudgetRoute {
                top_k: self
                    .config
                    .memory
                    .low_confidence_top_k
                    .min(max_memory_items),
                confidence: 0.0,
                reason: "classifier_fallback".into(),
            },
            rag: RetrievalBudgetRoute {
                top_k: self
                    .config
                    .rag
                    .low_confidence_top_k
                    .min(cap_u64_to_u32(max_rag_chunks)),
                confidence: 0.0,
                reason: "classifier_fallback".into(),
            },
            diagnostics: ContextRouteDiagnostics {
                taxonomy: self.config.taxonomy.clone(),
                top_labels: Vec::new(),
                fallback_used: true,
            },
        }
    }
}

fn route_retrieval(
    config: &RetrievalRouteConfig,
    positive: f64,
    negative: f64,
    margin: f64,
    max_top_k: u32,
    route_name: &str,
) -> RetrievalBudgetRoute {
    if negative >= config.negative_threshold && negative + margin > positive {
        return RetrievalBudgetRoute {
            top_k: 0,
            confidence: negative,
            reason: format!("{route_name}_negative"),
        };
    }
    if positive >= config.positive_threshold && positive >= negative + margin {
        return RetrievalBudgetRoute {
            top_k: max_top_k,
            confidence: positive,
            reason: format!("{route_name}_positive"),
        };
    }
    RetrievalBudgetRoute {
        top_k: config.low_confidence_top_k.min(max_top_k),
        confidence: positive.max(negative),
        reason: format!("{route_name}_low_confidence"),
    }
}

fn route_rag(
    config: &RagRouteConfig,
    positive: f64,
    negative: f64,
    margin: f64,
    max_top_k: u64,
) -> RetrievalBudgetRoute {
    let max_top_k = cap_u64_to_u32(max_top_k);
    if negative >= config.negative_threshold && negative + margin > positive {
        return RetrievalBudgetRoute {
            top_k: 0,
            confidence: negative,
            reason: "rag_negative".into(),
        };
    }
    if positive >= config.positive_threshold && positive >= negative + margin {
        return RetrievalBudgetRoute {
            top_k: max_top_k,
            confidence: positive,
            reason: "rag_positive".into(),
        };
    }
    RetrievalBudgetRoute {
        top_k: config.low_confidence_top_k.min(max_top_k),
        confidence: positive.max(negative),
        reason: "rag_low_confidence".into(),
    }
}

fn diagnostics_from_set(set: &SemanticClassificationSet) -> ContextRouteDiagnostics {
    ContextRouteDiagnostics {
        taxonomy: set.taxonomy.as_str().to_string(),
        top_labels: set
            .classifications
            .iter()
            .take(8)
            .map(|classification| {
                (
                    classification.label.as_str().to_string(),
                    classification.score,
                )
            })
            .collect(),
        fallback_used: set.fallback_used,
    }
}

pub fn build_routing_input(recent_messages: &[ChatMessage]) -> SemanticInput {
    let user_messages = recent_messages
        .iter()
        .filter(|message| message.role == "user")
        .collect::<Vec<_>>();
    let Some(latest) = user_messages.last() else {
        return SemanticInput::new("");
    };
    let primary_text = latest.content.clone();
    let auxiliary_texts = if needs_auxiliary_context(&primary_text) {
        user_messages
            .iter()
            .rev()
            .skip(1)
            .take(3)
            .map(|message| message.content.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        Vec::new()
    };

    SemanticInput {
        primary_text,
        auxiliary_texts,
        metadata: serde_json::Value::Null,
    }
}

fn needs_auxiliary_context(text: &str) -> bool {
    if text.chars().filter(|c| !c.is_whitespace()).count() < 12 {
        return true;
    }

    let lower = text.to_lowercase();
    const REFERENCES: &[&str] = &[
        "那",
        "这个",
        "刚才",
        "上面",
        "之前",
        "继续",
        "它",
        "他",
        "她",
        "这件事",
        "那个",
        "that",
        "it",
        "this",
        "continue",
        "previous",
    ];
    REFERENCES.iter().any(|word| lower.contains(word))
}

fn cap_u64_to_u32(value: u64) -> u32 {
    value.min(u64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use crate::domain::llm::ChatMessage;
    use crate::domain::semantic_classification::{
        SemanticClassification, SemanticClassificationError, SemanticClassificationSet,
        SemanticClassifierT, SemanticInput, SemanticLabel, SemanticTaxonomyId,
    };
    use crate::shared::config::ContextRoutingConfig;

    use super::*;

    struct FixedClassifier {
        set: Option<SemanticClassificationSet>,
        error: Option<String>,
    }

    #[async_trait]
    impl SemanticClassifierT for FixedClassifier {
        async fn classify(
            &self,
            _taxonomy: &SemanticTaxonomyId,
            _input: SemanticInput,
        ) -> Result<SemanticClassificationSet, SemanticClassificationError> {
            if let Some(error) = &self.error {
                Err(SemanticClassificationError::Provider(error.clone()))
            } else {
                Ok(self.set.clone().expect("fixed classifier set is required"))
            }
        }
    }

    fn set(scores: &[(&str, f64)]) -> SemanticClassificationSet {
        SemanticClassificationSet {
            taxonomy: SemanticTaxonomyId::new("context_routing"),
            classifications: scores
                .iter()
                .map(|(label, score)| SemanticClassification {
                    label: SemanticLabel::new(*label),
                    score: *score,
                    matched_prototype_ids: vec![label.to_string()],
                })
                .collect(),
            fallback_used: false,
        }
    }

    fn config() -> ContextRoutingConfig {
        let mut config = ContextRoutingConfig::default();
        config.enabled = true;
        config.memory.low_confidence_top_k = 2;
        config.rag.low_confidence_top_k = 1;
        config
    }

    #[tokio::test]
    async fn route_enables_fresh_and_full_memory_when_positive_scores_win() {
        let classifier = Arc::new(FixedClassifier {
            set: Some(set(&[
                ("context.fresh.positive", 0.9),
                ("context.fresh.negative", 0.1),
                ("context.memory.positive", 0.8),
                ("context.memory.negative", 0.1),
                ("context.rag.positive", 0.2),
                ("context.rag.negative", 0.1),
            ])),
            error: None,
        });
        let service = ContextRoutingService::new(classifier, config());

        let decision = service
            .route(SemanticInput::new("今天有什么新闻"), 10, 5)
            .await;

        assert!(decision.fresh_context.enabled);
        assert_eq!(decision.memory.top_k, 10);
        assert_eq!(decision.rag.top_k, 1);
        assert_eq!(decision.memory.reason, "memory_positive");
    }

    #[tokio::test]
    async fn classifier_failure_uses_conservative_fallback() {
        let classifier = Arc::new(FixedClassifier {
            set: None,
            error: Some("offline".into()),
        });
        let service = ContextRoutingService::new(classifier, config());

        let decision = service.route(SemanticInput::new("anything"), 10, 5).await;

        assert!(!decision.fresh_context.enabled);
        assert_eq!(decision.memory.top_k, 2);
        assert_eq!(decision.rag.top_k, 1);
        assert!(decision.diagnostics.fallback_used);
    }

    #[test]
    fn build_routing_input_adds_auxiliary_for_referential_short_turn() {
        let messages = vec![
            ChatMessage {
                role: "user".into(),
                content: "帮我设计语义路由".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: "可以".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "user".into(),
                content: "继续".into(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];

        let input = build_routing_input(&messages);

        assert_eq!(input.primary_text, "继续");
        assert_eq!(input.auxiliary_texts, vec!["帮我设计语义路由"]);
    }

    #[tokio::test]
    async fn rag_positive_not_suppressed_by_former_current_task_signal() {
        // 移除 current_task 抑制后，只要 rag_positive 过阈值就应保留 RAG 预算
        let classifier = Arc::new(FixedClassifier {
            set: Some(set(&[
                ("context.rag.positive", 0.75),
                ("context.rag.negative", 0.10),
            ])),
            error: None,
        });
        let service = ContextRoutingService::new(classifier, config());

        let decision = service.route(SemanticInput::new("请查知识库"), 10, 5).await;

        assert_eq!(decision.rag.reason, "rag_positive");
        assert_eq!(decision.rag.top_k, 5);
    }
}
