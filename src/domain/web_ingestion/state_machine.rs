//! State machine validation for `knowledge_ingestion_runs`.
//!
//! Every transition must pass `can_transition_run(from_status, from_stage,
//! to_status, to_stage) -> bool`. Replaying an already-reached target is
//! idempotent and must NOT be treated as an error.

use crate::domain::web_ingestion::status::*;

/// Validate a run (status, stage) transition.
///
/// Returning `true` means the transition is allowed.  Reaching the same
/// target is always allowed (idempotent).
pub fn can_transition_run(
    from_status: &str,
    from_stage: &str,
    to_status: &str,
    to_stage: &str,
) -> bool {
    // ── Idempotent: already at target ────────────────────────────────
    if from_status == to_status && from_stage == to_stage {
        return true;
    }

    // ── Terminal statuses block all further progression ──────────────
    if is_terminal_run_status(from_status) {
        return false;
    }

    // ── Any non-terminal can transition to a failure/cancel/dead ─────
    if to_status == run_status::FAILED && to_stage == run_stage::FAILED {
        return true;
    }
    if to_status == run_status::DEAD && to_stage == run_stage::DEAD {
        return true;
    }
    if to_status == run_status::CANCELLED && to_stage == run_stage::CANCELLED {
        return true;
    }

    // ── Main happy paths ─────────────────────────────────────────────
    match (from_status, from_stage, to_status, to_stage) {
        // pending → fetching
        (run_status::PENDING, run_stage::PENDING, run_status::RUNNING, run_stage::FETCHING) => true,
        // fetching → fetched
        (run_status::RUNNING, run_stage::FETCHING, run_status::RUNNING, run_stage::FETCHED) => true,
        // fetched → unchanged (skip)
        (run_status::RUNNING, run_stage::FETCHED, run_status::SKIPPED, run_stage::UNCHANGED) => {
            true
        }
        // fetched → cleaning
        (run_status::RUNNING, run_stage::FETCHED, run_status::RUNNING, run_stage::CLEANING) => true,
        // cleaning → cleaned
        (run_status::RUNNING, run_stage::CLEANING, run_status::RUNNING, run_stage::CLEANED) => true,
        // cleaned → distilling
        (run_status::RUNNING, run_stage::CLEANED, run_status::RUNNING, run_stage::DISTILLING) => {
            true
        }
        // distilling → distilled
        (run_status::RUNNING, run_stage::DISTILLING, run_status::RUNNING, run_stage::DISTILLED) => {
            true
        }
        // distilled → quality_checked
        (
            run_status::RUNNING,
            run_stage::DISTILLED,
            run_status::RUNNING,
            run_stage::QUALITY_CHECKED,
        ) => true,
        // quality_checked → rejected
        (
            run_status::RUNNING,
            run_stage::QUALITY_CHECKED,
            run_status::REJECTED,
            run_stage::REJECTED,
        ) => true,
        // fetched → rejected (content too short / unusable)
        (run_status::RUNNING, run_stage::FETCHED, run_status::REJECTED, run_stage::REJECTED) => {
            true
        }
        // quality_checked → chunking
        (
            run_status::RUNNING,
            run_stage::QUALITY_CHECKED,
            run_status::RUNNING,
            run_stage::CHUNKING,
        ) => true,
        // quality_checked → staging (when chunking/embedding pipeline not yet implemented)
        (
            run_status::RUNNING,
            run_stage::QUALITY_CHECKED,
            run_status::STAGED,
            run_stage::STAGING,
        ) => true,
        // chunking → chunked
        (run_status::RUNNING, run_stage::CHUNKING, run_status::RUNNING, run_stage::CHUNKED) => true,
        // chunked → embedding
        (run_status::RUNNING, run_stage::CHUNKED, run_status::RUNNING, run_stage::EMBEDDING) => {
            true
        }
        // embedding → embedded
        (run_status::RUNNING, run_stage::EMBEDDING, run_status::RUNNING, run_stage::EMBEDDED) => {
            true
        }
        // embedded → indexing
        (run_status::RUNNING, run_stage::EMBEDDED, run_status::RUNNING, run_stage::INDEXING) => {
            true
        }
        // indexing → indexed
        (run_status::RUNNING, run_stage::INDEXING, run_status::RUNNING, run_stage::INDEXED) => true,
        // indexed → staging
        (run_status::RUNNING, run_stage::INDEXED, run_status::STAGED, run_stage::STAGING) => true,
        // staging → publishing
        (run_status::STAGED, run_stage::STAGING, run_status::RUNNING, run_stage::PUBLISHING) => {
            true
        }
        // publishing → published
        (
            run_status::RUNNING,
            run_stage::PUBLISHING,
            run_status::PUBLISHED,
            run_stage::PUBLISHED,
        ) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Happy path ──────────────────────────────────────────────────────

    #[test]
    fn test_full_happy_path() {
        let path = [
            (run_status::PENDING, run_stage::PENDING),
            (run_status::RUNNING, run_stage::FETCHING),
            (run_status::RUNNING, run_stage::FETCHED),
            (run_status::RUNNING, run_stage::CLEANING),
            (run_status::RUNNING, run_stage::CLEANED),
            (run_status::RUNNING, run_stage::DISTILLING),
            (run_status::RUNNING, run_stage::DISTILLED),
            (run_status::RUNNING, run_stage::QUALITY_CHECKED),
            (run_status::RUNNING, run_stage::CHUNKING),
            (run_status::RUNNING, run_stage::CHUNKED),
            (run_status::RUNNING, run_stage::EMBEDDING),
            (run_status::RUNNING, run_stage::EMBEDDED),
            (run_status::RUNNING, run_stage::INDEXING),
            (run_status::RUNNING, run_stage::INDEXED),
            (run_status::STAGED, run_stage::STAGING),
            (run_status::RUNNING, run_stage::PUBLISHING),
            (run_status::PUBLISHED, run_stage::PUBLISHED),
        ];
        for w in path.windows(2) {
            let (fs, fg) = w[0];
            let (ts, tg) = w[1];
            assert!(
                can_transition_run(fs, fg, ts, tg),
                "transition ({fs},{fg}) -> ({ts},{tg}) should be valid"
            );
        }
    }

    // ── Unchanged / skip path ───────────────────────────────────────────

    #[test]
    fn test_unchanged_skip() {
        assert!(can_transition_run(
            run_status::RUNNING,
            run_stage::FETCHED,
            run_status::SKIPPED,
            run_stage::UNCHANGED,
        ));
    }

    // ── Rejected path ───────────────────────────────────────────────────

    #[test]
    fn test_quality_rejected() {
        assert!(can_transition_run(
            run_status::RUNNING,
            run_stage::QUALITY_CHECKED,
            run_status::REJECTED,
            run_stage::REJECTED,
        ));
    }

    // ── Idempotent ──────────────────────────────────────────────────────

    #[test]
    fn test_idempotent_same_state() {
        assert!(can_transition_run(
            run_status::RUNNING,
            run_stage::DISTILLING,
            run_status::RUNNING,
            run_stage::DISTILLING,
        ));
    }

    // ── Fail from any stage ─────────────────────────────────────────────

    #[test]
    fn test_fail_from_any_stage() {
        assert!(can_transition_run(
            run_status::RUNNING,
            run_stage::DISTILLING,
            run_status::FAILED,
            run_stage::FAILED,
        ));
    }

    // ── Terminal blocks further progress ────────────────────────────────

    #[test]
    fn test_published_is_terminal() {
        assert!(!can_transition_run(
            run_status::PUBLISHED,
            run_stage::PUBLISHED,
            run_status::RUNNING,
            run_stage::CHUNKING,
        ));
    }

    #[test]
    fn test_rejected_is_terminal() {
        assert!(!can_transition_run(
            run_status::REJECTED,
            run_stage::REJECTED,
            run_status::RUNNING,
            run_stage::CHUNKING,
        ));
    }

    // ── Illegal transitions ─────────────────────────────────────────────

    #[test]
    fn test_cannot_jump_stages() {
        assert!(!can_transition_run(
            run_status::PENDING,
            run_stage::PENDING,
            run_status::RUNNING,
            run_stage::DISTILLED, // skipped fetching, cleaning, distilling
        ));
    }
}
