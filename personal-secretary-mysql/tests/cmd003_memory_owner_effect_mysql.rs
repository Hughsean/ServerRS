//! CMD-003 记忆写命令的最终事务边界。
//!
//! 四类 Owner 记忆写 Action 必须在同一 MySQL 事务内复验 Action 租约、
//! OwnerCommand、active OwnerBinding 与账号，并原子提交业务变更和 Effect Receipt。

mod common;

use common::{
    account, drop_schema, insert_action_run, insert_group_message, isolated_db,
    owner_command_with_binding, scalar_string, scalar_u64,
};
use personal_secretary::{
    ActionLeaseToken, ActionRunId, Clock, ContentTrustLevel, MemoryEffectRequest,
    MemoryEffectStoreError, MemoryFact, MemoryFactId, MemoryFactStatus, MemoryPayload,
    MemoryUseCase, ProjectMemory, SecretaryAction, SecretaryActionProposal, SourceEventId,
    SystemClock, VerifiedActorKind,
};
use personal_secretary_mysql::{build_mysql_inbound_event_store, build_mysql_memory_store};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

#[tokio::test]
#[ignore]
async fn owner_memory_effects_are_atomic_fenced_and_idempotent() {
    let (db, schema) = isolated_db("_cmd003").await;
    let outcome = tokio::spawn(owner_memory_effect_scenario(db.clone())).await;
    drop_schema(&db, &schema).await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(message)) => panic!("CMD-003 memory effect scenario failed: {message}"),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
}

async fn owner_memory_effect_scenario(db: DatabaseConnection) -> Result<(), String> {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let managed = format!("cmd003-managed-{suffix}");
    let other_managed = format!("cmd003-other-{suffix}");
    let command_account = format!("cmd003-command-{suffix}");
    let managed_account = account(&managed);
    let other_account = account(&other_managed);
    let inbound = build_mysql_inbound_event_store(db.clone());
    let memory_store = build_mysql_memory_store(db.clone());
    let memory = MemoryUseCase::new(memory_store.clone());
    let now = SystemClock.now_unix_secs();

    let source = insert_group_message(
        &inbound,
        &managed,
        "cmd003-source",
        "cmd003-group",
        "cmd003-actor",
        VerifiedActorKind::External,
        now - 120,
        "source",
    )
    .await;
    let other_source = insert_group_message(
        &inbound,
        &other_managed,
        "cmd003-other-source",
        "cmd003-other-group",
        "cmd003-other-actor",
        VerifiedActorKind::External,
        now - 120,
        "other source",
    )
    .await;

    let correct_fact = seed_project_fact(&memory, &managed_account, "correct", &source).await;
    let ttl_fact = seed_project_fact(&memory, &managed_account, "ttl", &source).await;
    let delete_fact = seed_project_fact(&memory, &managed_account, "delete", &source).await;
    let other_fact = seed_project_fact(&memory, &other_account, "other", &other_source).await;

    let command = owner_command_with_binding(
        &db,
        &inbound,
        &managed,
        &command_account,
        "cmd003-owner-command",
        "管理记忆",
        now - 30,
    )
    .await;
    let run_id = ActionRunId::for_owner_command(&command, "v1");
    let lease = ActionLeaseToken::generate();
    insert_action_run(&db, &managed_account, &run_id, &command, &lease).await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_action_runs SET recent_events_json = \
         JSON_ARRAY(JSON_OBJECT('source_event_id', ?)) WHERE run_id = ?",
        [source.as_str().into(), run_id.as_str().into()],
    ))
    .await
    .map_err(|error| error.to_string())?;

    let correct = request(
        &managed_account,
        &command,
        &run_id,
        &lease,
        "cmd003-effect-correct",
        SecretaryAction::CorrectMemoryFact {
            fact_id: correct_fact.fact_id.clone(),
            replacement: project_payload("corrected"),
            confidence_bps: 9_900,
            source_event_ids: vec![source.clone()],
            valid_until_unix_secs: None,
        },
        now,
        &source,
    );
    let first_receipt = memory
        .apply_owner_effect(&correct)
        .await
        .map_err(|error| format!("correct: {error}"))?;
    let repeated_receipt = memory
        .apply_owner_effect(&correct)
        .await
        .map_err(|error| format!("correct replay: {error}"))?;
    assert_eq!(
        first_receipt, repeated_receipt,
        "exact replay is idempotent"
    );
    assert_eq!(
        fact_status(&db, &correct_fact.fact_id).await,
        "superseded",
        "correction supersedes the previous fact"
    );

    let collision = request(
        &managed_account,
        &command,
        &run_id,
        &lease,
        "cmd003-effect-correct",
        SecretaryAction::DeleteMemoryFact {
            fact_id: delete_fact.fact_id.clone(),
            reason: "collision".into(),
        },
        now,
        &source,
    );
    assert!(matches!(
        memory.apply_owner_effect(&collision).await,
        Err(MemoryEffectStoreError::InvalidData(_))
    ));

    let ttl = request(
        &managed_account,
        &command,
        &run_id,
        &lease,
        "cmd003-effect-ttl",
        SecretaryAction::SetMemoryFactTtl {
            fact_id: ttl_fact.fact_id.clone(),
            valid_until_unix_secs: Some(now + 86_400),
        },
        now,
        &source,
    );
    memory
        .apply_owner_effect(&ttl)
        .await
        .map_err(|error| format!("ttl: {error}"))?;
    assert_eq!(fact_status(&db, &ttl_fact.fact_id).await, "superseded");

    let delete = request(
        &managed_account,
        &command,
        &run_id,
        &lease,
        "cmd003-effect-delete",
        SecretaryAction::DeleteMemoryFact {
            fact_id: delete_fact.fact_id.clone(),
            reason: "Owner requested deletion".into(),
        },
        now,
        &source,
    );
    memory
        .apply_owner_effect(&delete)
        .await
        .map_err(|error| format!("delete: {error}"))?;
    assert_eq!(fact_status(&db, &delete_fact.fact_id).await, "deleted");

    let cross_account = request(
        &managed_account,
        &command,
        &run_id,
        &lease,
        "cmd003-effect-cross-account",
        SecretaryAction::DeleteMemoryFact {
            fact_id: other_fact.fact_id.clone(),
            reason: "must fail".into(),
        },
        now,
        &source,
    );
    assert!(matches!(
        memory.apply_owner_effect(&cross_account).await,
        Err(MemoryEffectStoreError::InvalidData(_))
    ));
    assert_eq!(fact_status(&db, &other_fact.fact_id).await, "confirmed");

    let conversation = personal_secretary::ConversationRef::new(
        personal_secretary::ConversationKind::Group,
        "cmd003-group",
    )
    .map_err(|error| error.to_string())?;
    let mode = request(
        &managed_account,
        &command,
        &run_id,
        &lease,
        "cmd003-effect-mode",
        SecretaryAction::SetConversationMemoryMode {
            conversation,
            mode: ContentTrustLevel::LocalOnly,
        },
        now,
        &source,
    );
    memory
        .apply_owner_effect(&mode)
        .await
        .map_err(|error| format!("mode: {error}"))?;
    assert_eq!(conversation_mode(&db, &managed).await, "local_only");
    assert_eq!(action_run_status(&db, &run_id).await, "running");
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_action_effect_receipts WHERE run_id = ?",
            vec![run_id.as_str().into()],
        )
        .await,
        4,
        "four successful memory effects produce exactly four receipts"
    );

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_action_runs SET lease_expires_at = UTC_TIMESTAMP(6) - INTERVAL 1 SECOND WHERE run_id = ?",
        [run_id.as_str().into()],
    ))
    .await
    .map_err(|error| error.to_string())?;
    let expired = request(
        &managed_account,
        &command,
        &run_id,
        &lease,
        "cmd003-effect-expired",
        SecretaryAction::SetConversationMemoryMode {
            conversation: personal_secretary::ConversationRef::new(
                personal_secretary::ConversationKind::Group,
                "cmd003-group",
            )
            .map_err(|error| error.to_string())?,
            mode: ContentTrustLevel::Normal,
        },
        now,
        &source,
    );
    assert!(matches!(
        memory.apply_owner_effect(&expired).await,
        Err(MemoryEffectStoreError::LeaseLost)
    ));
    assert_eq!(conversation_mode(&db, &managed).await, "local_only");

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_action_runs SET lease_expires_at = UTC_TIMESTAMP(6) + INTERVAL 60 SECOND WHERE run_id = ?",
        [run_id.as_str().into()],
    ))
    .await
    .map_err(|error| error.to_string())?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_owner_bindings SET status = 'revoked' WHERE managed_account_id = \
         (SELECT id FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?)",
        [managed.clone().into()],
    ))
    .await
    .map_err(|error| error.to_string())?;
    let revoked = request(
        &managed_account,
        &command,
        &run_id,
        &lease,
        "cmd003-effect-revoked",
        SecretaryAction::SetConversationMemoryMode {
            conversation: personal_secretary::ConversationRef::new(
                personal_secretary::ConversationKind::Group,
                "cmd003-group",
            )
            .map_err(|error| error.to_string())?,
            mode: ContentTrustLevel::Normal,
        },
        now,
        &source,
    );
    assert!(matches!(
        memory.apply_owner_effect(&revoked).await,
        Err(MemoryEffectStoreError::Unauthorized)
    ));
    assert_eq!(conversation_mode(&db, &managed).await, "local_only");
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_action_effect_receipts WHERE run_id = ?",
            vec![run_id.as_str().into()],
        )
        .await,
        4,
        "rejected effects create no receipt"
    );
    Ok(())
}

async fn seed_project_fact(
    memory: &MemoryUseCase,
    account: &personal_secretary::SourceAccountRef,
    key: &str,
    source: &SourceEventId,
) -> MemoryFact {
    let fact = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: account.clone(),
        subject_key: format!("project:{key}"),
        payload: project_payload(key),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_500,
        source_event_ids: vec![source.clone()],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory.remember(&fact).await.expect("seed memory fact");
    fact
}

fn project_payload(key: &str) -> MemoryPayload {
    MemoryPayload::Project(ProjectMemory {
        project_key: key.into(),
        goal: format!("goal-{key}"),
        member_actor_ids: Vec::new(),
        member_actor_refs: Vec::new(),
        progress: None,
        decision_ids: Vec::new(),
        risks: Vec::new(),
        blockers: Vec::new(),
        artifact_refs: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
fn request(
    account: &personal_secretary::SourceAccountRef,
    command: &SourceEventId,
    run_id: &ActionRunId,
    lease: &ActionLeaseToken,
    effect_id: &str,
    action: SecretaryAction,
    now_unix_secs: i64,
    source: &SourceEventId,
) -> MemoryEffectRequest {
    let proposal = SecretaryActionProposal::new(
        action.clone(),
        "CMD-003 memory effect",
        vec![source.clone()],
        Some(format!("cmd003:{effect_id}")),
    )
    .expect("valid memory proposal");
    MemoryEffectRequest {
        account: account.clone(),
        command_source_event_id: command.clone(),
        run_id: run_id.clone(),
        lease_token: lease.clone(),
        effect_id: effect_id.into(),
        proposal_id: proposal.proposal_id.clone(),
        proposal_json: serde_json::to_string(&proposal).expect("serialize proposal"),
        action,
        now_unix_secs,
    }
}

async fn fact_status(db: &DatabaseConnection, fact_id: &MemoryFactId) -> String {
    scalar_string(
        db,
        "SELECT fact_status AS value FROM secretary_memory_facts WHERE fact_id = ?",
        vec![fact_id.as_str().into()],
    )
    .await
}

async fn conversation_mode(db: &DatabaseConnection, managed: &str) -> String {
    scalar_string(
        db,
        "SELECT conversation.memory_mode AS value FROM secretary_conversations conversation \
         JOIN secretary_accounts account ON account.id = conversation.account_id \
         WHERE account.source_channel = 'napcat' AND account.platform_account_id = ? \
           AND conversation.conversation_kind = 'group' \
           AND conversation.platform_conversation_id = 'cmd003-group'",
        vec![managed.into()],
    )
    .await
}

async fn action_run_status(db: &DatabaseConnection, run_id: &ActionRunId) -> String {
    scalar_string(
        db,
        "SELECT status AS value FROM secretary_action_runs WHERE run_id = ?",
        vec![run_id.as_str().into()],
    )
    .await
}
