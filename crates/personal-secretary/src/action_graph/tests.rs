//! Action Graph 辅助函数与错误分类的单元测试。

use super::*;
use agent_core::graph::EffectErrorKind;

use crate::SourceEventId;

#[test]
fn backoff_first_attempt_is_base() {
    assert_eq!(backoff_ms(1, 500, 10_000), 500);
}

#[test]
fn backoff_doubles_each_attempt() {
    assert_eq!(backoff_ms(2, 500, 10_000), 1000);
    assert_eq!(backoff_ms(3, 500, 10_000), 2000);
}

#[test]
fn backoff_capped_at_max() {
    assert_eq!(backoff_ms(10, 500, 10_000), 10_000);
}

#[test]
fn backoff_saturates_on_huge_attempt() {
    assert_eq!(backoff_ms(u32::MAX, 500, 10_000), 10_000);
}

#[test]
fn l0_readonly_is_direct_execute() {
    assert!(is_l0_direct_execute(SecretaryRiskLevel::L0ReadOnly));
}

#[test]
fn l1_reversible_is_direct_execute() {
    assert!(is_l0_direct_execute(SecretaryRiskLevel::L1Reversible));
}

#[test]
fn l2_impactful_not_direct_execute() {
    assert!(!is_l0_direct_execute(SecretaryRiskLevel::L2Impactful));
}

#[test]
fn l3_external_not_direct_execute() {
    assert!(!is_l0_direct_execute(
        SecretaryRiskLevel::L3ExternalSideEffect
    ));
}

#[test]
fn action_run_id_rejects_empty() {
    assert!(ActionRunId::new("").is_err());
    assert!(ActionRunId::new("  ").is_err());
}

#[test]
fn action_run_id_accepts_non_empty() {
    assert!(ActionRunId::new("run-1").is_ok());
}

#[test]
fn action_ids_reject_database_truncation() {
    assert!(ActionRunId::new("x".repeat(37)).is_err());
    assert!(ActionLeaseToken::new("x".repeat(37)).is_err());
}

#[test]
fn owner_command_run_id_is_stable_uuid_and_version_scoped() {
    let source = SourceEventId::new("550e8400-e29b-41d4-a716-446655440000").unwrap();
    let first = ActionRunId::for_owner_command(&source, "v1");
    let repeated = ActionRunId::for_owner_command(&source, "v1");
    let upgraded = ActionRunId::for_owner_command(&source, "v2");
    assert_eq!(first, repeated);
    assert_ne!(first, upgraded);
    assert_eq!(first.as_str().len(), 36);
    assert!(uuid::Uuid::parse_str(first.as_str()).is_ok());
}

#[test]
fn action_lease_token_generates_uuid() {
    let token = ActionLeaseToken::generate();
    assert!(!token.as_str().is_empty());
}

#[test]
fn invalid_data_maps_to_permanent_effect_error() {
    let error = ActionStoreError::InvalidData("test".into());
    assert_eq!(error.to_effect_error().kind(), EffectErrorKind::Permanent);
}

#[test]
fn lease_lost_maps_to_permanent_effect_error() {
    let error = ActionStoreError::LeaseLost;
    assert_eq!(error.to_effect_error().kind(), EffectErrorKind::Permanent);
}

#[test]
fn database_error_maps_to_unknown_commit() {
    let error = ActionStoreError::Database("connection lost".into());
    assert_eq!(
        error.to_effect_error().kind(),
        EffectErrorKind::UnknownCommit
    );
}

#[test]
fn unavailable_maps_to_unknown_commit() {
    let error = ActionStoreError::Unavailable;
    assert_eq!(
        error.to_effect_error().kind(),
        EffectErrorKind::UnknownCommit
    );
}

#[test]
fn unknown_commit_maps_to_unknown_commit() {
    let error = ActionStoreError::UnknownCommit("maybe committed".into());
    assert_eq!(
        error.to_effect_error().kind(),
        EffectErrorKind::UnknownCommit
    );
}
