//! THR-010 线程逻辑迁移、语义重新确认与话题重开/撤销的真实 MySQL 场景。

mod common;

use personal_secretary::{
    ActionLeaseToken, ActionRunId, ConversationKind, ConversationRef, EventThreadId,
    InboundMessageEnvelope, MessageSource, SecretaryAction, SecretaryActionProposal,
    SourceAccountRef, SourceMessageRef, ThreadControlEffectRequest, ThreadMutationEffect,
    ThreadMutationImpact, ThreadMutationKind, ThreadMutationRevertInput, ThreadMutationUseCase,
    VerifiedActor, VerifiedActorKind,
};
use personal_secretary_mysql::{
    build_mysql_inbound_event_store, build_mysql_thread_control_store,
    build_mysql_thread_mutation_store,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn semantic_reconfirmation_and_split_revert_close_empty_thread() {
    let (db, schema) = common::isolated_db("_thr010").await;
    let scenario_db = db.clone();
    let result = tokio::spawn(async move { run_scenario(scenario_db).await }).await;
    common::drop_schema(&db, &schema).await;
    result.expect("THR-010 MySQL scenario must complete");
}

async fn run_scenario(db: DatabaseConnection) {
    let inbound = build_mysql_inbound_event_store(db.clone());
    let managed = "thr010-managed";
    let account = SourceAccountRef::new(MessageSource::NapCat, managed).unwrap();
    let merge_a = common::insert_group_message(
        &inbound,
        managed,
        "thr010-merge-a",
        "thr010-merge-group-a",
        "thr010-actor-a",
        VerifiedActorKind::External,
        2_000_000_001,
        "merge evidence A",
    )
    .await;
    let merge_b = common::insert_group_message(
        &inbound,
        managed,
        "thr010-merge-b",
        "thr010-merge-group-b",
        "thr010-actor-b",
        VerifiedActorKind::External,
        2_000_000_002,
        "merge evidence B",
    )
    .await;
    let split_a = common::insert_group_message(
        &inbound,
        managed,
        "thr010-split-a",
        "thr010-split-group",
        "thr010-actor-c",
        VerifiedActorKind::External,
        2_000_000_003,
        "split evidence A",
    )
    .await;
    let split_b = common::insert_group_message(
        &inbound,
        managed,
        "thr010-split-b",
        "thr010-split-group",
        "thr010-actor-d",
        VerifiedActorKind::External,
        2_000_000_004,
        "split evidence B",
    )
    .await;
    let split_c = common::insert_group_message(
        &inbound,
        managed,
        "thr010-split-c",
        "thr010-split-group",
        "thr010-actor-e",
        VerifiedActorKind::External,
        2_000_000_005,
        "split evidence retained in source",
    )
    .await;

    create_thread(&db, "thr010-merge-old", &merge_a, &merge_a, managed).await;
    create_thread(&db, "thr010-merge-new", &merge_b, &merge_b, managed).await;
    attach(&db, "thr010-merge-old", &merge_a).await;
    attach(&db, "thr010-merge-new", &merge_b).await;
    create_thread(&db, "thr010-split-source", &split_a, &split_c, managed).await;
    attach(&db, "thr010-split-source", &split_a).await;
    attach(&db, "thr010-split-source", &split_b).await;
    attach(&db, "thr010-split-source", &split_c).await;

    let decision_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_decisions (decision_id, thread_id, statement, status, confidence_bps) VALUES (?, 'thr010-merge-old', '人工确认的旧结论', 'confirmed', 9000)",
        [decision_id.clone().into()],
    ))
    .await
    .expect("confirmed decision fixture must persist");

    let mutation_store = build_mysql_thread_mutation_store(db.clone());
    let merge_proposal = uuid::Uuid::new_v4().to_string();
    let merge_command = common::owner_command_with_binding(
        &db,
        &inbound,
        managed,
        "thr010-owner",
        "thr010-action-merge",
        "合并两个线程",
        2_000_000_006,
    )
    .await;
    let merge_action = SecretaryAction::MergeThreads {
        thread_ids: vec![
            EventThreadId::new("thr010-merge-old").unwrap(),
            EventThreadId::new("thr010-merge-new").unwrap(),
        ],
        reason: "Owner 确认两个线程属于同一事项".into(),
    };
    ThreadMutationUseCase::new(mutation_store.clone())
        .apply_approved_action(
            &account,
            &merge_proposal,
            &merge_action,
            &merge_command,
            "thr010-merge-effect",
        )
        .await
        .expect("merge action must persist, authorize and apply");
    assert_eq!(pending_invalidations(&db, "thr010-merge-old").await, 1);

    apply_reconfirmation(
        &db,
        &inbound,
        &account,
        managed,
        "thr010-owner",
        "thr010-reconfirm",
        2_000_000_010,
        "thr010-reconfirm-effect",
        false,
    )
    .await
    .expect("Owner semantic reconfirmation must apply");
    assert_eq!(pending_invalidations(&db, "thr010-merge-old").await, 0);

    let no_pending = apply_reconfirmation(
        &db,
        &inbound,
        &account,
        managed,
        "thr010-owner",
        "thr010-reconfirm-noop",
        2_000_000_011,
        "thr010-reconfirm-noop-effect",
        false,
    )
    .await;
    assert!(
        matches!(
            no_pending,
            Err(personal_secretary::ThreadControlStoreError::InvalidData(_))
        ),
        "reconfirmation without a pending invalidation must fail closed: {no_pending:?}"
    );

    let newer_proposal = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_mutation_proposals \
         (proposal_id, account_id, mutation_kind, proposal_status, impact_json) \
         SELECT ?, id, 'merge', 'applied', JSON_OBJECT() FROM secretary_accounts \
         WHERE source_channel = 'napcat' AND platform_account_id = ?",
        [newer_proposal.clone().into(), managed.into()],
    ))
    .await
    .expect("newer mutation fixture must persist");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_semantic_invalidations \
         (invalidation_id, proposal_id, thread_id, invalidation_kind, created_at) \
         VALUES (?, ?, 'thr010-merge-old', 'mutation_applied', UTC_TIMESTAMP(6))",
        [Uuid::new_v4().to_string().into(), newer_proposal.into()],
    ))
    .await
    .expect("newer invalidation fixture must persist");
    assert_eq!(pending_invalidations(&db, "thr010-merge-old").await, 1);
    apply_reconfirmation(
        &db,
        &inbound,
        &account,
        managed,
        "thr010-owner",
        "thr010-reconfirm-newer",
        2_000_000_012,
        "thr010-reconfirm-newer-effect",
        false,
    )
    .await
    .expect("newer invalidation must require and accept a new reconfirmation");
    assert_eq!(pending_invalidations(&db, "thr010-merge-old").await, 0);

    let split_proposal = uuid::Uuid::new_v4().to_string();
    let split_impact = ThreadMutationImpact {
        proposal_id: personal_secretary::ThreadMutationProposalId::new(&split_proposal).unwrap(),
        kind: ThreadMutationKind::Split,
        account,
        thread_ids: vec![EventThreadId::new("thr010-split-source").unwrap()],
        affected_event_count: 2,
        affected_conversation_count: 1,
        affected_source_event_ids: vec![split_a, split_b],
        reason: "Owner 将线程中的一段独立出来".into(),
    };
    ThreadMutationUseCase::new(mutation_store.clone())
        .prepare(split_impact.clone())
        .await
        .expect("split proposal must persist");
    approve_proposal(&db, &split_proposal).await;
    mutation_store
        .apply_effect(
            &ThreadMutationEffect {
                proposal_id: split_impact.proposal_id.clone(),
                kind: ThreadMutationKind::Split,
            },
            "thr010-split-effect",
        )
        .await
        .expect("split effect must apply");
    assert_eq!(
        thread_status(&db, &split_proposal).await,
        Some("open".into())
    );

    let revert_command = insert_owner_command(
        &inbound,
        "thr010-owner",
        "thr010-revert",
        "撤销线程拆分",
        2_000_000_013,
    )
    .await;
    mutation_store
        .revert_applied(&ThreadMutationRevertInput {
            proposal_id: split_impact.proposal_id,
            command_source_event_id: revert_command,
            reason: "Owner 复核后撤销拆分".into(),
        })
        .await
        .expect("split revert must apply");
    assert_eq!(
        thread_status(&db, &split_proposal).await,
        Some("closed".into())
    );
    assert_eq!(pending_invalidations(&db, &split_proposal).await, 2);

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM qqbot_test_schema_migrations WHERE migration_name = ?",
        ["20260806_qqbot_thread_semantic_reconfirmation.sql".into()],
    ))
    .await
    .expect("remove THR-010 migration record for replay");
    common::try_replay_folded_migration(&db, "20260806_qqbot_thread_semantic_reconfirmation.sql")
        .await
        .expect("THR-010 migration must be safely replayable");
}

#[allow(clippy::too_many_arguments)]
async fn apply_reconfirmation(
    db: &DatabaseConnection,
    inbound: &std::sync::Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    account: &SourceAccountRef,
    managed: &str,
    command_account: &str,
    message_id: &str,
    occurred_at_unix_secs: i64,
    effect_id: &str,
    create_binding: bool,
) -> Result<personal_secretary::SecretaryActionReceipt, personal_secretary::ThreadControlStoreError>
{
    let command = if create_binding {
        common::owner_command_with_binding(
            db,
            inbound,
            managed,
            command_account,
            message_id,
            "重新确认线程语义",
            occurred_at_unix_secs,
        )
        .await
    } else {
        inbound
            .insert_message_if_absent(
                &InboundMessageEnvelope::new(
                    SourceMessageRef::new(
                        MessageSource::QqOpenPlatform,
                        command_account,
                        message_id,
                    )
                    .unwrap(),
                    ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                    VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                    occurred_at_unix_secs,
                    "重新确认线程语义",
                    Vec::new(),
                )
                .unwrap(),
            )
            .await
            .expect("insert subsequent OwnerCommand")
            .source_event_id()
            .clone()
    };
    let run_id = ActionRunId::for_owner_command(&command, effect_id);
    let lease = ActionLeaseToken::generate();
    common::insert_action_run(db, account, &run_id, &command, &lease).await;
    let action = SecretaryAction::ReconfirmThreadSemantics {
        thread_id: EventThreadId::new("thr010-merge-old").unwrap(),
        reason: "Owner 复核迁移后的既有结论".into(),
    };
    let proposal = SecretaryActionProposal::new(
        action.clone(),
        "测试线程语义重新确认",
        vec![command.clone()],
        Some(effect_id.into()),
    )
    .unwrap();
    build_mysql_thread_control_store(db.clone())
        .apply_effect(&ThreadControlEffectRequest {
            account: account.clone(),
            command_source_event_id: command,
            run_id,
            lease_token: lease,
            effect_id: effect_id.into(),
            proposal_id: proposal.proposal_id.clone(),
            proposal_json: serde_json::to_string(&proposal).unwrap(),
            action,
        })
        .await
}

async fn create_thread(
    db: &DatabaseConnection,
    thread_id: &str,
    root: &personal_secretary::SourceEventId,
    latest: &personal_secretary::SourceEventId,
    managed: &str,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_event_threads (thread_id, account_id, status, root_event_id, latest_event_id, opened_at_unix_secs, latest_occurred_at_unix_secs) SELECT ?, id, 'open', ?, ?, 2000000000, 2000000000 FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
        [thread_id.into(), root.as_str().into(), latest.as_str().into(), managed.into()],
    ))
    .await
    .expect("thread fixture must persist");
}

async fn attach(
    db: &DatabaseConnection,
    thread_id: &str,
    event: &personal_secretary::SourceEventId,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_events (source_event_id, thread_id) VALUES (?, ?)",
        [event.as_str().into(), thread_id.into()],
    ))
    .await
    .expect("thread member fixture must persist");
}

async fn approve_proposal(db: &DatabaseConnection, proposal_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_thread_mutation_proposals SET proposal_status = 'approved' WHERE proposal_id = ?",
        [proposal_id.into()],
    ))
    .await
    .expect("proposal must be approved");
}

async fn insert_owner_command(
    inbound: &std::sync::Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    command_account: &str,
    message_id: &str,
    text: &str,
    occurred_at_unix_secs: i64,
) -> personal_secretary::SourceEventId {
    inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::QqOpenPlatform, command_account, message_id)
                    .unwrap(),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                occurred_at_unix_secs,
                text,
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .expect("insert OwnerCommand")
        .source_event_id()
        .clone()
}

async fn pending_invalidations(db: &DatabaseConnection, thread_id: &str) -> u64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT COUNT(*) AS value FROM secretary_thread_semantic_invalidations invalidation WHERE invalidation.thread_id = ? AND NOT EXISTS (SELECT 1 FROM secretary_thread_semantic_reconfirmations reconfirmation WHERE reconfirmation.thread_id = invalidation.thread_id AND reconfirmation.created_at >= invalidation.created_at)",
            [thread_id.into()],
        ))
        .await
        .expect("invalidation query must succeed")
        .expect("invalidation count must return");
    u64::try_from(row.try_get::<i64>("", "value").unwrap()).unwrap()
}

async fn thread_status(db: &DatabaseConnection, thread_id: &str) -> Option<String> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT status AS value FROM secretary_event_threads WHERE thread_id = ?",
        [thread_id.into()],
    ))
    .await
    .expect("thread status query must succeed")
    .and_then(|row| row.try_get("", "value").ok())
}
