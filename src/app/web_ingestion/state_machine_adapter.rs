//! Centralized state-machine transitions for ingestion runs.
//!
//! Task-book §5.7: every status jump goes through one place; the return value
//! of `update_status_stage` is never silently swallowed. A `false` result means
//! the CAS failed (concurrent worker, already-advanced, or illegal) and the
//! caller MUST decide explicitly — it must NOT emit downstream events.

use std::sync::Arc;

use crate::domain::web_ingestion::error::WebIngestionError;
use crate::domain::web_ingestion::repo::IngestionRunRepoT;

/// Outcome of an attempted transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionOutcome {
    /// The transition was applied (expected state matched, transition legal).
    Applied,
    /// The expected (status, stage) did not match the current row — another
    /// worker advanced it, or this is an idempotent replay. The caller should
    /// treat this as "someone else owns the next step" and NOT emit downstream.
    NotApplied,
}

impl TransitionOutcome {
    pub fn applied(self) -> bool {
        matches!(self, TransitionOutcome::Applied)
    }
}

/// Attempt a guarded transition. Returns `Applied` only when the row actually
/// moved from the expected state to the new state. An illegal transition
/// (per `can_transition_run`) surfaces as `Err(InvalidTransition)` from the repo.
pub async fn transition(
    run_repo: &Arc<dyn IngestionRunRepoT>,
    run_id: u64,
    expected_status: &str,
    expected_stage: &str,
    new_status: &str,
    new_stage: &str,
    last_error: Option<&str>,
) -> Result<TransitionOutcome, WebIngestionError> {
    let applied = run_repo
        .update_status_stage(
            run_id,
            expected_status,
            expected_stage,
            new_status,
            new_stage,
            last_error,
        )
        .await?;
    Ok(if applied {
        TransitionOutcome::Applied
    } else {
        TransitionOutcome::NotApplied
    })
}

/// Classifies where a run sits relative to a handler's expected entry stage.
/// This is the core of idempotent resume (§5.8): a handler must distinguish
/// entry / mid / done / too-early / impossible rather than blindly skipping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagePosition {
    /// Run is at the handler's entry (status, stage) — execute normally.
    AtEntry,
    /// Run is at the handler's mid stage — resume the remaining work.
    Mid,
    /// Run has already reached the handler's target stage or a later one —
    /// idempotent success, nothing to do.
    Done,
    /// Run is in a terminal state (rejected / failed / dead / skipped /
    /// superseded / rolled_back) — idempotent success, branch ended.
    Terminal,
    /// Run sits before the handler's entry — a prerequisite event has not been
    /// processed yet; the handler should fail so the event is retried.
    TooEarly,
}
