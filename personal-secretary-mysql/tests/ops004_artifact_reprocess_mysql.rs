//! OPS-004 失败 Artifact 派生任务有界重处理 MySQL 验收。
//!
//! 默认忽略；需要 QQBOT_TEST_DATABASE_URL 指向 qqbot_accept_ 前缀的隔离基库。

mod common;

use common::{
    account, drop_schema, insert_action_run, insert_group_message, isolated_db,
    owner_command_with_binding, scalar_string, scalar_u64, try_apply_qqbot_migrations,
};
use personal_secretary::{
    ActionLeaseToken, ActionRunId, ArtifactReprocessEffectRequest, ArtifactReprocessStoreError,
    ArtifactReprocessUseCase, Clock, SecretaryAction, SecretaryActionProposal, SourceEventId,
    SystemClock, VerifiedActorKind,
};
use personal_secretary_mysql::{
    build_mysql_artifact_reprocess_store, build_mysql_inbound_event_store,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn owner_requeues_failed_artifact_derivations_with_bounds_fencing_and_audit() {
    let (db, schema) = isolated_db("_ops004").await;
    let outcome = tokio::spawn(run_scenario(db.clone())).await;
    drop_schema(&db, &schema).await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(message)) => panic!("OPS-004 scenario must pass: {message}"),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
}

async fn run_scenario(db: DatabaseConnection) -> Result<(), String> {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let managed_a = format!("ops004-a-{suffix}");
    let managed_b = format!("ops004-b-{suffix}");
    let command_account = format!("ops004-command-{suffix}");
    let inbound = build_mysql_inbound_event_store(db.clone());
    let now = SystemClock.now_unix_secs();

    let mut account_a_events = Vec::new();
    for ordinal in 0..3 {
        account_a_events.push(
            insert_group_message(
                &inbound,
                &managed_a,
                &format!("ops004-a-{ordinal}"),
                "group-a",
                "member-a",
                VerifiedActorKind::External,
                now + ordinal,
                "测试消息",
            )
            .await,
        );
    }
    let account_b_event = insert_group_message(
        &inbound,
        &managed_b,
        "ops004-b-0",
        "group-b",
        "member-b",
        VerifiedActorKind::External,
        now,
        "另一个账号的测试消息",
    )
    .await;
    mark_failed(&db, &account_a_events, "invalid_segments_json").await?;
    mark_failed(
        &db,
        std::slice::from_ref(&account_b_event),
        "invalid_segments_json",
    )
    .await?;

    let command_event = owner_command_with_binding(
        &db,
        &inbound,
        &managed_a,
        &command_account,
        "ops004-owner-command",
        "重试两个失败的产物派生任务",
        now + 10,
    )
    .await;
    let run_id = ActionRunId::for_owner_command(&command_event, "ops004-v1");
    let lease_token = ActionLeaseToken::generate();
    insert_action_run(
        &db,
        &account(&managed_a),
        &run_id,
        &command_event,
        &lease_token,
    )
    .await;

    let action = SecretaryAction::RetryFailedArtifactDerivations {
        limit: 2,
        reason: "Owner 确认重试故障修复后的任务".into(),
    };
    let proposal = SecretaryActionProposal::new(
        action.clone(),
        "重试有界失败队列",
        vec![command_event.clone()],
        Some(format!("ops004:{}", command_event.as_str())),
    )
    .map_err(|error| error.to_string())?;
    let proposal_json = serde_json::to_string(&proposal).map_err(|error| error.to_string())?;
    let request = ArtifactReprocessEffectRequest {
        account: account(&managed_a),
        command_source_event_id: command_event.clone(),
        run_id: run_id.clone(),
        lease_token: lease_token.clone(),
        effect_id: format!("ops004-effect-{suffix}"),
        proposal_id: proposal.proposal_id.clone(),
        proposal_json,
        action,
    };
    let use_case = ArtifactReprocessUseCase::new(build_mysql_artifact_reprocess_store(db.clone()));
    let receipt = use_case
        .apply_effect(&request)
        .await
        .map_err(|error| error.to_string())?;
    let result: serde_json::Value =
        serde_json::from_str(&receipt.result_ref).map_err(|error| error.to_string())?;
    if result["requested"] != 2 || result["requeued"] != 2 {
        return Err(format!("unexpected receipt: {}", receipt.result_ref));
    }

    assert_job_counts(&db, &managed_a, 2, 1).await?;
    assert_job_counts(&db, &managed_b, 0, 1).await?;
    let first_two_pending = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_artifact_derivations \
         WHERE source_event_id IN (?, ?) AND status = 'pending'",
        vec![
            account_a_events[0].as_str().into(),
            account_a_events[1].as_str().into(),
        ],
    )
    .await;
    if first_two_pending != 2 {
        return Err("stable oldest ordering did not requeue the first two jobs".into());
    }
    if scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_artifact_reprocess_audit WHERE effect_id = ? \
         AND requested_limit = 2 AND requeued_count = 2",
        vec![request.effect_id.clone().into()],
    )
    .await
        != 1
    {
        return Err("exactly one audit row must be committed".into());
    }
    let audited_targets: Vec<String> = serde_json::from_str(
        &scalar_string(
            &db,
            "SELECT CAST(requeued_source_event_ids AS CHAR) AS value \
         FROM secretary_artifact_reprocess_audit WHERE effect_id = ?",
            vec![request.effect_id.clone().into()],
        )
        .await,
    )
    .map_err(|error| error.to_string())?;
    if audited_targets
        != vec![
            account_a_events[0].as_str().to_owned(),
            account_a_events[1].as_str().to_owned(),
        ]
    {
        return Err("audit must preserve the exact stable requeue target order".into());
    }
    if scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_action_effect_receipts WHERE effect_id = ?",
        vec![request.effect_id.clone().into()],
    )
    .await
        != 1
    {
        return Err("exactly one effect receipt must be committed".into());
    }

    let replay = use_case
        .apply_effect(&request)
        .await
        .map_err(|error| error.to_string())?;
    if replay.result_ref != receipt.result_ref {
        return Err("effect replay must return the original receipt".into());
    }
    assert_job_counts(&db, &managed_a, 2, 1).await?;
    if scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_artifact_reprocess_audit WHERE effect_id = ?",
        vec![request.effect_id.clone().into()],
    )
    .await
        != 1
    {
        return Err("effect replay must not duplicate audit".into());
    }

    let mut forged = request.clone();
    forged.effect_id = format!("ops004-forged-{suffix}");
    forged.lease_token = ActionLeaseToken::generate();
    let forged_error = use_case
        .apply_effect(&forged)
        .await
        .expect_err("forged lease must be rejected");
    if !matches!(forged_error, ArtifactReprocessStoreError::LeaseLost) {
        return Err(format!("unexpected forged lease error: {forged_error}"));
    }
    assert_job_counts(&db, &managed_a, 2, 1).await?;

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_action_runs SET lease_expires_at = UTC_TIMESTAMP(6) - INTERVAL 1 SECOND \
         WHERE run_id = ?",
        vec![run_id.as_str().into()],
    ))
    .await
    .map_err(|error| error.to_string())?;
    let mut expired = request.clone();
    expired.effect_id = format!("ops004-expired-{suffix}");
    let expired_error = use_case
        .apply_effect(&expired)
        .await
        .expect_err("expired lease must be rejected");
    if !matches!(expired_error, ArtifactReprocessStoreError::LeaseLost) {
        return Err(format!("unexpected expired lease error: {expired_error}"));
    }
    assert_job_counts(&db, &managed_a, 2, 1).await?;

    try_apply_qqbot_migrations(&db)
        .await
        .map_err(|error| format!("migration replay failed: {error}"))?;
    Ok(())
}

async fn mark_failed(
    db: &DatabaseConnection,
    event_ids: &[SourceEventId],
    error_code: &str,
) -> Result<(), String> {
    for (ordinal, event_id) in event_ids.iter().enumerate() {
        let updated = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_artifact_derivations \
                 SET status = 'failed', attempts = 1, lease_token = NULL, lease_expires_at = NULL, \
                     last_error_code = ?, updated_at = FROM_UNIXTIME(?) \
                 WHERE source_event_id = ?",
                vec![
                    error_code.into(),
                    (1_700_000_000_i64 + ordinal as i64).into(),
                    event_id.as_str().into(),
                ],
            ))
            .await
            .map_err(|error| error.to_string())?;
        if updated.rows_affected() != 1 {
            return Err("artifact derivation fixture must exist".into());
        }
    }
    Ok(())
}

async fn assert_job_counts(
    db: &DatabaseConnection,
    managed_id: &str,
    expected_pending: u64,
    expected_failed: u64,
) -> Result<(), String> {
    let pending = scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_artifact_derivations job \
         JOIN secretary_source_events event ON event.source_event_id = job.source_event_id \
         JOIN secretary_accounts account ON account.id = event.account_id \
         WHERE account.source_channel = 'napcat' AND account.platform_account_id = ? \
           AND job.status = 'pending'",
        vec![managed_id.into()],
    )
    .await;
    let failed = scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_artifact_derivations job \
         JOIN secretary_source_events event ON event.source_event_id = job.source_event_id \
         JOIN secretary_accounts account ON account.id = event.account_id \
         WHERE account.source_channel = 'napcat' AND account.platform_account_id = ? \
           AND job.status = 'failed'",
        vec![managed_id.into()],
    )
    .await;
    if (pending, failed) != (expected_pending, expected_failed) {
        return Err(format!(
            "unexpected job counts for managed account: pending={pending}, failed={failed}"
        ));
    }
    Ok(())
}
