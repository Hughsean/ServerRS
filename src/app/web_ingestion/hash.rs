//! Deterministic idempotency-key computation.
//!
//! Every key is a SHA-256 hex string (CHAR(64)). These keys guarantee that
//! replaying an event or re-running a pipeline stage never produces duplicates.

use sha2::{Digest, Sha256};

fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// `canonical_url` hash: sha256(canonical_url)
pub fn url_hash(canonical_url: &str) -> String {
    sha256_hex(canonical_url)
}

/// `content_key`: sha256(source_id + page_id + content_hash)
pub fn content_key(source_id: u64, page_id: u64, content_hash: &str) -> String {
    sha256_hex(&format!("{source_id}|{page_id}|{content_hash}"))
}

/// `run_key` = sha256(
///   source_id + page_id + content_hash +
///   llm_prompt_version + chunker_version + embedding_model + pipeline_version
/// )
pub fn run_key(
    source_id: u64,
    page_id: u64,
    content_hash: &str,
    llm_prompt_version: &str,
    chunker_version: &str,
    embedding_model: &str,
    pipeline_version: &str,
) -> String {
    sha256_hex(&format!(
        "{source_id}|{page_id}|{content_hash}|{llm_prompt_version}|{chunker_version}|{embedding_model}|{pipeline_version}"
    ))
}

/// `version_key` = run_key
pub fn version_key(rk: &str) -> String {
    rk.to_string()
}

/// `event_key` (standard): sha256(event_type + aggregate_type + aggregate_id + run_id + version_key)
pub fn event_key(
    event_type: &str,
    aggregate_type: &str,
    aggregate_id: u64,
    run_id: u64,
    version_key: &str,
) -> String {
    sha256_hex(&format!(
        "{event_type}|{aggregate_type}|{aggregate_id}|{run_id}|{version_key}"
    ))
}

/// `event_key` for rollback: sha256("KnowledgeRollbackRequested" + current_record_id + target_record_id)
pub fn rollback_event_key(current_record_id: u64, target_record_id: u64) -> String {
    sha256_hex(&format!(
        "KnowledgeRollbackRequested|{current_record_id}|{target_record_id}"
    ))
}

/// `chunk_hash`: sha256(version_key + chunk_type + chunk_index + normalized_chunk_content + chunker_version)
pub fn chunk_hash(
    version_key: &str,
    chunk_type: &str,
    chunk_index: u32,
    normalized_content: &str,
    chunker_version: &str,
) -> String {
    sha256_hex(&format!(
        "{version_key}|{chunk_type}|{chunk_index}|{normalized_content}|{chunker_version}"
    ))
}

/// `vector_point_id`: sha256(index_name + chunk_hash + embedding_model)
pub fn vector_point_id(index_name: &str, chunk_hash: &str, embedding_model: &str) -> String {
    sha256_hex(&format!("{index_name}|{chunk_hash}|{embedding_model}"))
}

/// Simple SHA-256 of raw content (used for dedup detection).
pub fn content_hash(raw: &str) -> String {
    sha256_hex(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_hash_deterministic() {
        let h1 = content_hash("hello");
        let h2 = content_hash("hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_run_key_different_on_input_change() {
        let k1 = run_key(1, 2, "abc", "v1", "c1", "m1", "p1");
        let k2 = run_key(1, 2, "abc", "v2", "c1", "m1", "p1"); // different prompt version
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_chunk_hash_includes_version() {
        let h1 = chunk_hash("vk1", "atomic", 0, "text", "cv1");
        let h2 = chunk_hash("vk2", "atomic", 0, "text", "cv1"); // different version_key
        assert_ne!(h1, h2);
    }
}
