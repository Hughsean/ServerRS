use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::llm::EmbeddingProvider;
use crate::domain::semantic_classification::{
    SemanticClassification, SemanticClassificationError, SemanticClassificationSet,
    SemanticClassifierT, SemanticInput, SemanticLabel, SemanticTaxonomyId,
};
use crate::shared::config::SemanticClassificationConfig;

#[derive(Debug, Clone)]
struct EmbeddedPrototype {
    id: String,
    label: SemanticLabel,
    vector: Vec<f32>,
    weight: f64,
}

pub struct EmbeddingSemanticClassifier {
    embedding_provider: Arc<dyn EmbeddingProvider>,
    prototypes_by_taxonomy: HashMap<String, Vec<EmbeddedPrototype>>,
}

impl EmbeddingSemanticClassifier {
    pub async fn from_config(
        config: &SemanticClassificationConfig,
        embedding_provider: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self, SemanticClassificationError> {
        let mut prototypes_by_taxonomy = HashMap::new();

        for taxonomy in &config.taxonomies {
            let taxonomy_id = taxonomy.id.trim().to_string();
            if taxonomy.prototypes.is_empty() {
                return Err(SemanticClassificationError::EmptyPrototypeSet(taxonomy_id));
            }

            let texts = taxonomy
                .prototypes
                .iter()
                .map(|prototype| prototype.text.clone())
                .collect::<Vec<_>>();
            let vectors = embedding_provider
                .embed(&texts)
                .await
                .map_err(|error| SemanticClassificationError::Provider(error.to_string()))?;

            if vectors.len() != taxonomy.prototypes.len() {
                return Err(SemanticClassificationError::Provider(format!(
                    "embedding provider returned {} vectors for {} prototypes",
                    vectors.len(),
                    taxonomy.prototypes.len()
                )));
            }

            let prototypes = taxonomy
                .prototypes
                .iter()
                .zip(vectors)
                .map(|(prototype, vector)| EmbeddedPrototype {
                    id: prototype.id.clone(),
                    label: SemanticLabel::new(prototype.label.clone()),
                    vector,
                    weight: prototype.weight,
                })
                .collect::<Vec<_>>();
            prototypes_by_taxonomy.insert(taxonomy_id, prototypes);
        }

        Ok(Self {
            embedding_provider,
            prototypes_by_taxonomy,
        })
    }
}

#[async_trait]
impl SemanticClassifierT for EmbeddingSemanticClassifier {
    async fn classify(
        &self,
        taxonomy: &SemanticTaxonomyId,
        input: SemanticInput,
    ) -> Result<SemanticClassificationSet, SemanticClassificationError> {
        let prototypes = self
            .prototypes_by_taxonomy
            .get(taxonomy.as_str())
            .ok_or_else(|| {
                SemanticClassificationError::UnknownTaxonomy(taxonomy.as_str().to_string())
            })?;
        if prototypes.is_empty() {
            return Err(SemanticClassificationError::EmptyPrototypeSet(
                taxonomy.as_str().to_string(),
            ));
        }
        if input.is_empty() {
            return Ok(SemanticClassificationSet::empty(taxonomy.clone()));
        }

        let query_text = classification_text(&input);
        let mut query_vectors = self
            .embedding_provider
            .embed(&[query_text])
            .await
            .map_err(|error| SemanticClassificationError::Provider(error.to_string()))?;
        let query_vector = query_vectors.pop().ok_or_else(|| {
            SemanticClassificationError::Provider("embedding provider returned no vectors".into())
        })?;

        let mut best_by_label: HashMap<SemanticLabel, SemanticClassification> = HashMap::new();
        for prototype in prototypes {
            let score = (cosine_similarity(&query_vector, &prototype.vector).max(0.0)
                * prototype.weight)
                .min(1.0);
            match best_by_label.get_mut(&prototype.label) {
                Some(existing) if existing.score >= score => {}
                Some(existing) => {
                    existing.score = score;
                    existing.matched_prototype_ids = vec![prototype.id.clone()];
                }
                None => {
                    best_by_label.insert(
                        prototype.label.clone(),
                        SemanticClassification {
                            label: prototype.label.clone(),
                            score,
                            matched_prototype_ids: vec![prototype.id.clone()],
                        },
                    );
                }
            }
        }

        let mut classifications = best_by_label.into_values().collect::<Vec<_>>();
        classifications.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.label.as_str().cmp(b.label.as_str()))
        });

        Ok(SemanticClassificationSet {
            taxonomy: taxonomy.clone(),
            classifications,
            fallback_used: false,
        })
    }
}

fn classification_text(input: &SemanticInput) -> String {
    std::iter::once(input.primary_text.as_str())
        .chain(input.auxiliary_texts.iter().map(String::as_str))
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0_f64;
    let mut left_norm = 0.0_f64;
    let mut right_norm = 0.0_f64;
    for (l, r) in left.iter().zip(right.iter()) {
        let l = f64::from(*l);
        let r = f64::from(*r);
        dot += l * r;
        left_norm += l * l;
        right_norm += r * r;
    }

    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm.sqrt() * right_norm.sqrt())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;

    use crate::domain::llm::{EmbeddingProvider, LlmError};
    use crate::domain::semantic_classification::{
        SemanticClassificationError, SemanticClassifierT, SemanticInput, SemanticTaxonomyId,
    };
    use crate::shared::config::{
        SemanticClassificationConfig, SemanticPrototypeConfig, SemanticTaxonomyConfig,
    };

    use super::*;

    struct DeterministicEmbeddingProvider {
        vectors: HashMap<String, Vec<f32>>,
    }

    #[async_trait]
    impl EmbeddingProvider for DeterministicEmbeddingProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
            texts
                .iter()
                .map(|text| {
                    self.vectors.get(text).cloned().ok_or_else(|| {
                        LlmError::EmbeddingError(format!("missing vector for {text}"))
                    })
                })
                .collect()
        }
    }

    fn provider() -> Arc<dyn EmbeddingProvider> {
        Arc::new(DeterministicEmbeddingProvider {
            vectors: HashMap::from([
                ("recent news".into(), vec![1.0, 0.0]),
                ("stable code".into(), vec![0.0, 1.0]),
                ("need current facts".into(), vec![1.0, 0.0]),
            ]),
        })
    }

    fn config() -> SemanticClassificationConfig {
        SemanticClassificationConfig {
            enabled: true,
            provider: "embedding".into(),
            taxonomies: vec![SemanticTaxonomyConfig {
                id: "context_routing".into(),
                prototypes: vec![
                    SemanticPrototypeConfig {
                        id: "fresh".into(),
                        label: "context.fresh.positive".into(),
                        text: "recent news".into(),
                        weight: 1.0,
                    },
                    SemanticPrototypeConfig {
                        id: "stable".into(),
                        label: "context.fresh.negative".into(),
                        text: "stable code".into(),
                        weight: 1.0,
                    },
                ],
            }],
        }
    }

    #[tokio::test]
    async fn classifies_by_highest_weighted_prototype_similarity() {
        let classifier = EmbeddingSemanticClassifier::from_config(&config(), provider())
            .await
            .unwrap();

        let set = classifier
            .classify(
                &SemanticTaxonomyId::new("context_routing"),
                SemanticInput::new("need current facts"),
            )
            .await
            .unwrap();

        assert_eq!(set.score_for("context.fresh.positive"), 1.0);
        assert_eq!(set.score_for("context.fresh.negative"), 0.0);
        assert_eq!(
            set.classifications[0].matched_prototype_ids,
            vec!["fresh".to_string()]
        );
    }

    #[tokio::test]
    async fn unknown_taxonomy_returns_domain_error() {
        let classifier = EmbeddingSemanticClassifier::from_config(&config(), provider())
            .await
            .unwrap();

        let error = classifier
            .classify(
                &SemanticTaxonomyId::new("missing"),
                SemanticInput::new("need current facts"),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            SemanticClassificationError::UnknownTaxonomy(id) if id == "missing"
        ));
    }
}
