//! Run-key / version-key construction (task-book §5.6).
//!
//! `run_key` is the full idempotency key over content + profile. Two runs with
//! the same content but different embedding model / prompt version / chunker
//! version / pipeline version produce DIFFERENT run_keys, so the same content
//! is reprocessed under a new profile. `content_key` is only a content-hash
//! index and must NOT block reprocessing across profiles.

use crate::application::web_ingestion::hash;
use crate::application::web_ingestion::services::run_profile::RunProfile;

/// Compute the full run_key for a (page, content, profile) tuple.
pub fn build_run_key(
    source_id: u64,
    page_id: u64,
    content_hash: &str,
    profile: &RunProfile,
) -> String {
    hash::run_key(
        source_id,
        page_id,
        content_hash,
        &profile.llm_prompt_version,
        &profile.chunker_version,
        &profile.embedding_model,
        &profile.pipeline_version,
    )
}

/// version_key currently equals run_key (one published version per run). Kept
/// as a separate function so the two can diverge without touching call sites.
pub fn build_version_key(run_key: &str) -> String {
    hash::version_key(run_key)
}

/// content_key — content-hash index only (NOT a uniqueness gate across profiles).
pub fn build_content_key(source_id: u64, page_id: u64, content_hash: &str) -> String {
    hash::content_key(source_id, page_id, content_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(model: &str, prompt: &str, chunker: &str, pipeline: &str) -> RunProfile {
        RunProfile {
            llm_prompt_version: prompt.into(),
            chunker_version: chunker.into(),
            embedding_provider: "ollama".into(),
            embedding_model: model.into(),
            embedding_dimension: 768,
            pipeline_version: pipeline.into(),
        }
    }

    #[test]
    fn run_key_changes_with_embedding_model() {
        let a = build_run_key(1, 2, "h", &profile("m1", "p1", "c1", "v1"));
        let b = build_run_key(1, 2, "h", &profile("m2", "p1", "c1", "v1"));
        assert_ne!(
            a, b,
            "different embedding_model must yield different run_key"
        );
    }

    #[test]
    fn run_key_changes_with_prompt_version() {
        let a = build_run_key(1, 2, "h", &profile("m1", "p1", "c1", "v1"));
        let b = build_run_key(1, 2, "h", &profile("m1", "p2", "c1", "v1"));
        assert_ne!(a, b);
    }

    #[test]
    fn run_key_changes_with_chunker_version() {
        let a = build_run_key(1, 2, "h", &profile("m1", "p1", "c1", "v1"));
        let b = build_run_key(1, 2, "h", &profile("m1", "p1", "c2", "v1"));
        assert_ne!(a, b);
    }

    #[test]
    fn run_key_changes_with_pipeline_version() {
        let a = build_run_key(1, 2, "h", &profile("m1", "p1", "c1", "v1"));
        let b = build_run_key(1, 2, "h", &profile("m1", "p1", "c1", "v2"));
        assert_ne!(a, b);
    }

    #[test]
    fn content_key_stable_across_profiles() {
        // content_key must NOT depend on profile — same content → same key.
        let a = build_content_key(1, 2, "h");
        let b = build_content_key(1, 2, "h");
        assert_eq!(a, b);
        // but run_key under two profiles differs, proving content_key does not
        // block reprocessing.
        let rk_a = build_run_key(1, 2, "h", &profile("m1", "p1", "c1", "v1"));
        let rk_b = build_run_key(1, 2, "h", &profile("m2", "p1", "c1", "v1"));
        assert_ne!(rk_a, rk_b);
    }
}
