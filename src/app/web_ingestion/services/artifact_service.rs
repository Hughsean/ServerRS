//! Artifact persistence service (task-book §6, §4.5).
//!
//! Centralizes reading/writing pipeline artifacts on the ingestion run row:
//! raw fetched body, clean text, distilled JSON, and the stable quality result.
//! Large text lives in `knowledge_ingestion_runs` columns (MEDIUMTEXT/JSON),
//! NEVER in the outbox payload (hard constraint #18).

use std::sync::Arc;

use serde_json::Value as JsonValue;

use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repository::IngestionRunRepository;

/// Persist the raw fetched body for a run.
pub async fn save_fetched_body(
    run_repo: &Arc<dyn IngestionRunRepository>,
    run_id: u64,
    body: &str,
) -> Result<(), WebIngestionError> {
    run_repo
        .update_artifacts(run_id, Some(body), None, None)
        .await
}

/// Persist the cleaned text for a run.
pub async fn save_clean_text(
    run_repo: &Arc<dyn IngestionRunRepository>,
    run_id: u64,
    clean_text: &str,
) -> Result<(), WebIngestionError> {
    run_repo
        .update_artifacts(run_id, None, Some(clean_text), None)
        .await
}

/// Persist the distilled JSON for a run.
pub async fn save_distilled(
    run_repo: &Arc<dyn IngestionRunRepository>,
    run_id: u64,
    distilled: JsonValue,
) -> Result<(), WebIngestionError> {
    run_repo
        .update_artifacts(run_id, None, None, Some(distilled))
        .await
}
