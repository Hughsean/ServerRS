use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ContextRoutingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_taxonomy")]
    pub taxonomy: String,
    #[serde(default = "default_margin")]
    pub margin: f64,
    #[serde(default)]
    pub fresh_context: FreshContextRouteConfig,
    #[serde(default)]
    pub memory: RetrievalRouteConfig,
    #[serde(default)]
    pub rag: RagRouteConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FreshContextRouteConfig {
    #[serde(default = "default_fresh_positive_threshold")]
    pub positive_threshold: f64,
    #[serde(default = "default_fresh_negative_threshold")]
    pub negative_threshold: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetrievalRouteConfig {
    #[serde(default = "default_retrieval_positive_threshold")]
    pub positive_threshold: f64,
    #[serde(default = "default_retrieval_negative_threshold")]
    pub negative_threshold: f64,
    #[serde(default = "default_low_confidence_top_k")]
    pub low_confidence_top_k: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RagRouteConfig {
    #[serde(default = "default_rag_positive_threshold")]
    pub positive_threshold: f64,
    #[serde(default = "default_retrieval_negative_threshold")]
    pub negative_threshold: f64,
    #[serde(default = "default_rag_low_confidence_top_k")]
    pub low_confidence_top_k: u32,
    #[serde(default)]
    pub current_task_top_k: u32,
}

impl Default for ContextRoutingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            taxonomy: default_taxonomy(),
            margin: default_margin(),
            fresh_context: FreshContextRouteConfig::default(),
            memory: RetrievalRouteConfig::default(),
            rag: RagRouteConfig::default(),
        }
    }
}

impl Default for FreshContextRouteConfig {
    fn default() -> Self {
        Self {
            positive_threshold: default_fresh_positive_threshold(),
            negative_threshold: default_fresh_negative_threshold(),
        }
    }
}

impl Default for RetrievalRouteConfig {
    fn default() -> Self {
        Self {
            positive_threshold: default_retrieval_positive_threshold(),
            negative_threshold: default_retrieval_negative_threshold(),
            low_confidence_top_k: default_low_confidence_top_k(),
        }
    }
}

impl Default for RagRouteConfig {
    fn default() -> Self {
        Self {
            positive_threshold: default_rag_positive_threshold(),
            negative_threshold: default_retrieval_negative_threshold(),
            low_confidence_top_k: default_rag_low_confidence_top_k(),
            current_task_top_k: 0,
        }
    }
}

impl ContextRoutingConfig {
    pub fn validate(&self) -> Result<(), String> {
        validate_threshold("context_routing.margin", self.margin)?;
        validate_threshold(
            "context_routing.fresh_context.positive_threshold",
            self.fresh_context.positive_threshold,
        )?;
        validate_threshold(
            "context_routing.fresh_context.negative_threshold",
            self.fresh_context.negative_threshold,
        )?;
        validate_threshold(
            "context_routing.memory.positive_threshold",
            self.memory.positive_threshold,
        )?;
        validate_threshold(
            "context_routing.memory.negative_threshold",
            self.memory.negative_threshold,
        )?;
        validate_threshold(
            "context_routing.rag.positive_threshold",
            self.rag.positive_threshold,
        )?;
        validate_threshold(
            "context_routing.rag.negative_threshold",
            self.rag.negative_threshold,
        )?;
        if self.taxonomy.trim().is_empty() {
            return Err("context_routing.taxonomy must not be empty".into());
        }
        Ok(())
    }
}

fn validate_threshold(field: &str, value: f64) -> Result<(), String> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(format!("{field} must be between 0.0 and 1.0"))
    }
}

fn default_taxonomy() -> String {
    "context_routing".into()
}
fn default_margin() -> f64 {
    0.05
}
fn default_fresh_positive_threshold() -> f64 {
    0.72
}
fn default_fresh_negative_threshold() -> f64 {
    0.72
}
fn default_retrieval_positive_threshold() -> f64 {
    0.68
}
fn default_retrieval_negative_threshold() -> f64 {
    0.72
}
fn default_low_confidence_top_k() -> u32 {
    3
}
fn default_rag_positive_threshold() -> f64 {
    0.66
}
fn default_rag_low_confidence_top_k() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_routing_defaults_disabled() {
        let config = ContextRoutingConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.taxonomy, "context_routing");
        assert_eq!(config.rag.current_task_top_k, 0);
    }

    #[test]
    fn rejects_threshold_outside_unit_range() {
        let mut config = ContextRoutingConfig::default();
        config.fresh_context.positive_threshold = 1.2;

        let error = config.validate().unwrap_err();
        assert!(error.contains("context_routing.fresh_context.positive_threshold"));
    }
}
