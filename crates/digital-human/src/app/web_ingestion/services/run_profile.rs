//! Run profile — the set of versioned parameters that define a pipeline run's
//! identity. Centralizing this (task-book §5.6) guarantees run_key/version_key
//! always use the REAL embedding model, prompt version, chunker version, and
//! pipeline version — never a placeholder like "embedding_default".

use crate::app::web_ingestion::pipeline_context::PipelineContext;

/// The versioned profile that distinguishes one processing run from another.
#[derive(Debug, Clone)]
pub struct RunProfile {
    pub llm_prompt_version: String,
    pub chunker_version: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_dimension: usize,
    pub pipeline_version: String,
}

impl RunProfile {
    /// Build the run profile from the live pipeline context. Validates that the
    /// embedding model is non-empty (§5.6: "embedding_default" / empty is
    /// forbidden as a final value).
    pub fn from_context(ctx: &PipelineContext) -> Result<Self, String> {
        let embedding_model = ctx.embedding_model().trim().to_string();
        if embedding_model.is_empty() {
            return Err(
                "embedding_model is empty — set [embedding].model or EMBEDDING_MODEL".into(),
            );
        }
        if embedding_model == "embedding_default" {
            return Err(
                "embedding_model is the placeholder 'embedding_default' — configure a real model"
                    .into(),
            );
        }
        let prompt = ctx.llm_prompt_version().trim().to_string();
        let chunker = ctx.chunker_version().trim().to_string();
        let pipeline = ctx.pipeline_version().trim().to_string();
        if prompt.is_empty() || chunker.is_empty() || pipeline.is_empty() {
            return Err(
                "llm_prompt_version / chunker_version / pipeline_version must be set".into(),
            );
        }
        Ok(Self {
            llm_prompt_version: prompt,
            chunker_version: chunker,
            embedding_provider: ctx.embedding_provider_name().to_string(),
            embedding_model,
            embedding_dimension: ctx.embedding_dimension(),
            pipeline_version: pipeline,
        })
    }
}
