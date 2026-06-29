use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SemanticClassificationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub taxonomies: Vec<SemanticTaxonomyConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SemanticTaxonomyConfig {
    pub id: String,
    #[serde(default)]
    pub prototypes: Vec<SemanticPrototypeConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SemanticPrototypeConfig {
    pub id: String,
    pub label: String,
    pub text: String,
    #[serde(default = "default_weight")]
    pub weight: f64,
}

impl Default for SemanticClassificationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_provider(),
            taxonomies: Vec::new(),
        }
    }
}

impl SemanticClassificationConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if !self.provider.eq_ignore_ascii_case("embedding") {
            return Err("semantic_classification.provider must be embedding".into());
        }
        for taxonomy in &self.taxonomies {
            let taxonomy_id = taxonomy.id.trim();
            if taxonomy_id.is_empty() {
                return Err("semantic_classification.taxonomies.id must not be empty".into());
            }
            for prototype in &taxonomy.prototypes {
                let prototype_id = prototype.id.trim();
                if prototype_id.is_empty() {
                    return Err(format!(
                        "semantic_classification.taxonomies.{taxonomy_id}.prototypes.id must not be empty"
                    ));
                }
                if prototype.label.trim().is_empty() {
                    return Err(format!(
                        "semantic_classification.taxonomies.{taxonomy_id}.prototypes.{prototype_id}.label must not be empty"
                    ));
                }
                if prototype.text.trim().is_empty() {
                    return Err(format!(
                        "semantic_classification.taxonomies.{taxonomy_id}.prototypes.{prototype_id}.text must not be empty"
                    ));
                }
                if !prototype.weight.is_finite() || prototype.weight <= 0.0 {
                    return Err(format!(
                        "semantic_classification.taxonomies.{taxonomy_id}.prototypes.{prototype_id}.weight must be greater than 0"
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn taxonomy(&self, id: &str) -> Option<&SemanticTaxonomyConfig> {
        self.taxonomies.iter().find(|taxonomy| taxonomy.id == id)
    }
}

fn default_provider() -> String {
    "embedding".into()
}

fn default_weight() -> f64 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_classification_defaults_disabled() {
        let config = SemanticClassificationConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.provider, "embedding");
        assert!(config.taxonomies.is_empty());
    }

    #[test]
    fn rejects_empty_prototype_text_when_enabled() {
        let config = SemanticClassificationConfig {
            enabled: true,
            provider: "embedding".into(),
            taxonomies: vec![SemanticTaxonomyConfig {
                id: "context_routing".into(),
                prototypes: vec![SemanticPrototypeConfig {
                    id: "p1".into(),
                    label: "context.fresh.positive".into(),
                    text: " ".into(),
                    weight: 1.0,
                }],
            }],
        };

        let error = config.validate().unwrap_err();
        assert!(
            error.contains("semantic_classification.taxonomies.context_routing.prototypes.p1.text")
        );
    }
}
