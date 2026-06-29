use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SemanticTaxonomyId(String);

impl SemanticTaxonomyId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SemanticLabel(String);

impl SemanticLabel {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticInput {
    pub primary_text: String,
    pub auxiliary_texts: Vec<String>,
    pub metadata: serde_json::Value,
}

impl SemanticInput {
    pub fn new(primary_text: impl Into<String>) -> Self {
        Self {
            primary_text: primary_text.into(),
            auxiliary_texts: Vec::new(),
            metadata: serde_json::Value::Null,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.primary_text.trim().is_empty()
            && self
                .auxiliary_texts
                .iter()
                .all(|text| text.trim().is_empty())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticPrototype {
    pub id: String,
    pub taxonomy: SemanticTaxonomyId,
    pub label: SemanticLabel,
    pub text: String,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticClassification {
    pub label: SemanticLabel,
    pub score: f64,
    pub matched_prototype_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticClassificationSet {
    pub taxonomy: SemanticTaxonomyId,
    pub classifications: Vec<SemanticClassification>,
    pub fallback_used: bool,
}

impl SemanticClassificationSet {
    pub fn empty(taxonomy: SemanticTaxonomyId) -> Self {
        Self {
            taxonomy,
            classifications: Vec::new(),
            fallback_used: false,
        }
    }

    pub fn score_for(&self, label: &str) -> f64 {
        self.classifications
            .iter()
            .find(|classification| classification.label.as_str() == label)
            .map(|classification| classification.score)
            .unwrap_or(0.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SemanticClassificationError {
    #[error("semantic taxonomy not found: {0}")]
    UnknownTaxonomy(String),
    #[error("semantic taxonomy has no prototypes: {0}")]
    EmptyPrototypeSet(String),
    #[error("semantic classifier provider error: {0}")]
    Provider(String),
}

#[async_trait]
pub trait SemanticClassifierT: Send + Sync {
    async fn classify(
        &self,
        taxonomy: &SemanticTaxonomyId,
        input: SemanticInput,
    ) -> Result<SemanticClassificationSet, SemanticClassificationError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_input_reports_empty_when_primary_and_auxiliary_are_blank() {
        let input = SemanticInput {
            primary_text: "   ".into(),
            auxiliary_texts: vec![" ".into()],
            metadata: serde_json::Value::Null,
        };

        assert!(input.is_empty());
    }

    #[test]
    fn classification_set_finds_score_by_label() {
        let set = SemanticClassificationSet {
            taxonomy: SemanticTaxonomyId::new("context_routing"),
            classifications: vec![SemanticClassification {
                label: SemanticLabel::new("context.fresh.positive"),
                score: 0.81,
                matched_prototype_ids: vec!["p1".into()],
            }],
            fallback_used: false,
        };

        assert_eq!(set.score_for("context.fresh.positive"), 0.81);
        assert_eq!(set.score_for("context.memory.positive"), 0.0);
    }
}
