//! THR-009 cross-conversation retrieval privacy and derived-state invalidation.

mod common;

use personal_secretary::{
    ActionLeaseToken, ActionRunId, ActionStoreError, ContentTrustLevel, ConversationKind,
    ConversationMemoryModeInput, ConversationRef, EventQuery, EventRelationKind, MemoryUseCase,
    RetrieverPolicy, RetrieverUseCase, ThreadLinkReviewUseCase, VerifiedActorKind,
};
use personal_secretary_mysql::{
    build_mysql_action_store, build_mysql_inbound_event_store, build_mysql_memory_candidate_store,
    build_mysql_memory_store, build_mysql_retriever_store, build_mysql_thread_link_store,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn restricted_conversation_is_filtered_before_limit_and_invalidates_derivations() {
    let (db, schema) = common::isolated_db("_thr009").await;
    let scenario_db = db.clone();
    let result = tokio::spawn(async move { run_scenario(scenario_db).await }).await;
    common::drop_schema(&db, &schema).await;
    result.expect("THR-009 MySQL scenario must complete");
}

async fn run_scenario(db: DatabaseConnection) {
    let inbound = build_mysql_inbound_event_store(db.clone());
    let managed_id = "thr009-managed";
    let visible_a = common::insert_group_message(
        &inbound,
        managed_id,
        "thr009-visible-a",
        "thr009-visible-group",
        "thr009-visible-actor",
        VerifiedActorKind::External,
        1_900_000_001,
        "THR009-keyword 可见较早证据",
    )
    .await;
    let visible_b = common::insert_group_message(
        &inbound,
        managed_id,
        "thr009-visible-b",
        "thr009-visible-group",
        "thr009-visible-actor",
        VerifiedActorKind::External,
        1_900_000_002,
        "THR009-keyword 可见最新证据",
    )
    .await;
    let restricted = common::insert_group_message(
        &inbound,
        managed_id,
        "thr009-restricted",
        "thr009-restricted-group",
        "thr009-restricted-actor",
        VerifiedActorKind::External,
        1_900_000_003,
        "THR009-keyword 绝不能进入远程结果",
    )
    .await;
    seed_threads_and_derivations(&db, &visible_a, &visible_b, &restricted).await;

    let command = common::owner_command_with_binding(
        &db,
        &inbound,
        managed_id,
        "thr009-command-account",
        "thr009-command",
        "将受限群设置为仅本地",
        1_900_000_010,
    )
    .await;
    seed_action_state(&db, &visible_a, &visible_b, &restricted).await;
    let account = common::account(managed_id);
    let memory = MemoryUseCase::new(build_mysql_memory_store(db.clone()));
    let receipt = memory
        .set_conversation_mode(&ConversationMemoryModeInput {
            account: account.clone(),
            conversation: ConversationRef::new(ConversationKind::Group, "thr009-restricted-group")
                .unwrap(),
            command_source_event_id: command,
            mode: ContentTrustLevel::LocalOnly,
        })
        .await
        .expect("authorized visibility downgrade must succeed");

    assert!(receipt.changed);
    assert_eq!(receipt.invalidated.semantic_claims, 1);
    assert_eq!(receipt.invalidated.semantic_decisions, 1);
    assert_eq!(receipt.invalidated.open_questions, 1);
    assert_eq!(receipt.invalidated.response_expectations, 1);
    assert_eq!(receipt.invalidated.thread_link_candidates, 1);
    assert_eq!(receipt.invalidated.memory_candidates, 1);
    assert_eq!(receipt.invalidated.memory_facts, 1);
    assert_eq!(receipt.invalidated.follow_ups, 1);
    assert_eq!(receipt.invalidated.participant_profiles, 1);
    assert_eq!(receipt.invalidated.participant_observations, 1);
    assert_eq!(receipt.invalidated.owner_response_drafts, 1);
    assert_eq!(receipt.invalidated.revoked_action_runs, 1);
    assert_eq!(receipt.invalidated.reopened_threads, 1);
    assert_eq!(receipt.invalidated.revoked_worker_leases, 3);

    let action_store = build_mysql_action_store(db.clone());
    let lease_error = action_store
        .mark_completed(
            &ActionRunId::new("thr009-running-action").unwrap(),
            &ActionLeaseToken::new("thr009-action-lease").unwrap(),
            None,
        )
        .await
        .expect_err("a run holding evidence from the restricted conversation must lose its lease");
    assert!(matches!(lease_error, ActionStoreError::LeaseLost));
    assert_eq!(
        common::scalar_string(
            &db,
            "SELECT status AS value FROM secretary_action_runs WHERE run_id = 'thr009-running-action'",
            vec![],
        )
        .await,
        "failed"
    );
    assert_eq!(
        common::scalar_u64(
            &db,
            "SELECT invalidated AS value FROM secretary_action_responses WHERE response_id = 'thr009-response'",
            vec![],
        )
        .await,
        1
    );
    assert_eq!(
        common::scalar_string(
            &db,
            "SELECT JSON_UNQUOTE(JSON_EXTRACT(response_json, '$.invalidated')) AS value FROM secretary_action_responses WHERE response_id = 'thr009-response'",
            vec![],
        )
        .await,
        "true"
    );

    let retriever = RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    );
    let mut query = EventQuery::for_account(account.clone());
    query.query_text = Some("THR009-keyword".into());
    query.limit = 1;
    let remote_results = retriever
        .search_events(&query, false)
        .await
        .expect("remote event search must succeed");
    assert_eq!(remote_results.len(), 1);
    assert_eq!(remote_results[0].source_event_id, visible_b);
    assert!(!remote_results[0].excerpt.contains("绝不能进入"));

    let recent = retriever
        .list_recent_event_views(&account, 1, false)
        .await
        .expect("remote recent window must succeed");
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].source_event_id, visible_b);

    let local_retriever = RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy {
            allow_local_only_to_loopback_llm: true,
        },
    );
    let local_results = local_retriever
        .search_events(&query, true)
        .await
        .expect("explicit local-only search must succeed");
    assert_eq!(local_results.len(), 1);
    assert_eq!(local_results[0].source_event_id, restricted);

    assert!(
        retriever
            .read_source_event_for_model(&restricted, &account, false)
            .await
            .expect("normal-only event lookup must succeed")
            .is_none(),
        "direct event lookup must not bypass candidate-stage authorization"
    );
    let local_detail = local_retriever
        .read_source_event_for_model(&restricted, &account, true)
        .await
        .expect("verified loopback event lookup must succeed")
        .expect("local-only content is visible to the explicitly allowed loopback model");
    assert!(local_detail.normalized_text.contains("绝不能进入远程结果"));
    assert!(
        retriever
            .event_causal_context(&account, &restricted)
            .await
            .expect("causal lookup must succeed")
            .is_none(),
        "causal lookup by ID must not reveal a restricted event"
    );
    let visible_causal = retriever
        .event_causal_context(&account, &visible_b)
        .await
        .expect("visible causal lookup must succeed")
        .expect("visible event must remain readable");
    assert!(visible_causal.reply_parent.is_none());
    assert!(
        visible_causal
            .relations
            .iter()
            .all(|relation| relation.kind != EventRelationKind::RepliesTo)
    );
    assert!(!visible_causal.source_refs.contains(&restricted));
    assert!(
        retriever
            .participant_context(
                &account,
                "thr009-restricted-actor",
                Some(
                    &ConversationRef::new(ConversationKind::Group, "thr009-restricted-group",)
                        .unwrap()
                ),
                None,
            )
            .await
            .expect("participant lookup must succeed")
            .is_none(),
        "participant lookup must require current normal evidence"
    );
    assert!(
        retriever
            .participants_by_display_name(
                &account,
                "受限昵称",
                Some(
                    &ConversationRef::new(ConversationKind::Group, "thr009-restricted-group",)
                        .unwrap()
                ),
                None,
                5,
            )
            .await
            .expect("participant name lookup must succeed")
            .is_empty(),
        "restricted participant profile must not remain a name-resolution candidate"
    );

    let candidate_store = build_mysql_memory_candidate_store(db.clone());
    assert!(
        candidate_store
            .list_candidates(&account, None, None, 1)
            .await
            .expect("memory candidate list must succeed")
            .is_empty(),
        "candidate payloads backed by restricted sources must be filtered before LIMIT"
    );

    let thread_id = personal_secretary::EventThreadId::new("thr009-thread-b").unwrap();
    let context = retriever
        .thread_context(&account, &thread_id)
        .await
        .expect("thread context query must succeed")
        .expect("mixed thread keeps visible evidence");
    assert_eq!(context.status, personal_secretary::ThreadStatus::Reopened);
    assert_eq!(context.claims.len(), 1);
    assert_eq!(context.claims[0].statement, "可见要求仍保留");
    assert!(context.decisions.is_empty());
    assert!(context.open_questions.is_empty());
    assert_eq!(context.event_count, 1);

    let revisions = retriever
        .thread_decision_revisions(&account, &thread_id, None, 10)
        .await
        .expect("decision revision query must succeed");
    assert!(revisions.decisions.is_empty());

    let links = ThreadLinkReviewUseCase::new(build_mysql_thread_link_store(db.clone()))
        .list_pending(&account, 10)
        .await
        .expect("pending link query must succeed");
    assert!(links.is_empty());
    assert!(memory.active(&account, 10).await.unwrap().is_empty());

    assert_status(
        &db,
        "secretary_thread_claims",
        "claim_id",
        "thr009-secret-claim",
        "status",
        "withdrawn",
    )
    .await;
    assert_status(
        &db,
        "secretary_thread_decisions",
        "decision_id",
        "thr009-secret-decision",
        "status",
        "revoked",
    )
    .await;
    assert_status(
        &db,
        "secretary_thread_open_questions",
        "question_id",
        "thr009-secret-question",
        "status",
        "dismissed",
    )
    .await;
    assert_status(
        &db,
        "secretary_memory_candidates",
        "candidate_id",
        "thr009-memory-candidate",
        "candidate_status",
        "invalidated",
    )
    .await;
    assert_status(
        &db,
        "secretary_memory_facts",
        "fact_id",
        "thr009-memory-fact",
        "fact_status",
        "expired",
    )
    .await;
    assert_status(
        &db,
        "secretary_follow_up_items",
        "follow_up_id",
        "thr009-follow-up",
        "status",
        "superseded",
    )
    .await;
}

async fn seed_action_state(
    db: &DatabaseConnection,
    visible_a: &personal_secretary::SourceEventId,
    visible_b: &personal_secretary::SourceEventId,
    restricted: &personal_secretary::SourceEventId,
) {
    exec(
        db,
        r#"INSERT INTO secretary_action_runs
           (run_id, account_id, command_source_event_id, command_text, conversation_id,
            occurred_at_unix_secs, timezone_offset_secs, timezone_name, recent_events_json,
            status, worker_id, lease_token, lease_expires_at)
           SELECT 'thr009-running-action', account_id, ?, 'running action', 'owner-control',
                  1900000011, 0, 'UTC',
                  JSON_ARRAY(JSON_OBJECT('source_event_id', ?)),
                  'running', 'thr009-worker', 'thr009-action-lease',
                  UTC_TIMESTAMP(6) + INTERVAL 60 SECOND
           FROM secretary_source_events WHERE source_event_id = ?"#,
        vec![
            visible_a.as_str().into(),
            restricted.as_str().into(),
            restricted.as_str().into(),
        ],
    )
    .await;
    let draft = serde_json::json!({
        "segments": [{"kind": "summary", "text": "受限草稿"}],
        "source_event_ids": [restricted.as_str()],
        "created_at_unix_secs": 1_900_000_012_i64,
        "invalidated": false
    })
    .to_string();
    exec(
        db,
        r#"INSERT INTO secretary_action_runs
           (run_id, account_id, command_source_event_id, command_text, conversation_id,
            occurred_at_unix_secs, timezone_offset_secs, timezone_name, recent_events_json,
            status, response_draft_json, completed_at)
           SELECT 'thr009-completed-action', account_id, ?, 'completed action', 'owner-control',
                  1900000012, 0, 'UTC', JSON_ARRAY(), 'completed', ?, UTC_TIMESTAMP(6)
           FROM secretary_source_events WHERE source_event_id = ?"#,
        vec![
            visible_b.as_str().into(),
            draft.clone().into(),
            restricted.as_str().into(),
        ],
    )
    .await;
    exec(
        db,
        r#"INSERT INTO secretary_action_responses
           (response_id, run_id, response_json, serialized_bytes, invalidated)
           VALUES ('thr009-response', 'thr009-completed-action', ?, ?, FALSE)"#,
        vec![draft.clone().into(), (draft.len() as u64).into()],
    )
    .await;
}

async fn seed_threads_and_derivations(
    db: &DatabaseConnection,
    visible_a: &personal_secretary::SourceEventId,
    visible_b: &personal_secretary::SourceEventId,
    restricted: &personal_secretary::SourceEventId,
) {
    exec(
        db,
        r#"INSERT INTO secretary_event_threads
           (thread_id, account_id, status, root_event_id, latest_event_id,
            opened_at_unix_secs, latest_occurred_at_unix_secs)
           SELECT 'thr009-thread-a', account_id, 'open', source_event_id, source_event_id,
                  occurred_at_unix_secs, occurred_at_unix_secs
           FROM secretary_source_events WHERE source_event_id = ?"#,
        vec![visible_a.as_str().into()],
    )
    .await;
    exec(
        db,
        r#"INSERT INTO secretary_event_threads
           (thread_id, account_id, status, root_event_id, latest_event_id,
            opened_at_unix_secs, latest_occurred_at_unix_secs)
           SELECT 'thr009-thread-b', account_id, 'resolved', source_event_id, ?,
                  occurred_at_unix_secs, 1900000003
           FROM secretary_source_events WHERE source_event_id = ?"#,
        vec![restricted.as_str().into(), visible_b.as_str().into()],
    )
    .await;
    for (event, thread) in [
        (visible_a, "thr009-thread-a"),
        (visible_b, "thr009-thread-b"),
        (restricted, "thr009-thread-b"),
    ] {
        exec(
            db,
            "INSERT INTO secretary_thread_events (source_event_id, thread_id) VALUES (?, ?)",
            vec![event.as_str().into(), thread.into()],
        )
        .await;
    }
    exec(
        db,
        "UPDATE secretary_source_events SET reply_to_event_id = ? WHERE source_event_id = ?",
        vec![restricted.as_str().into(), visible_b.as_str().into()],
    )
    .await;

    exec(
        db,
        r#"INSERT INTO secretary_thread_claims
        (claim_id, thread_id, claim_kind, claimant_channel, claimant_account,
         claimant_actor_id, statement, status, confidence_bps)
        VALUES ('thr009-visible-claim', 'thr009-thread-b', 'request', 'napcat',
                'thr009-managed', 'thr009-visible-actor', '可见要求仍保留', 'confirmed', 9000),
               ('thr009-secret-claim', 'thr009-thread-b', 'request', 'napcat',
                'thr009-managed', 'thr009-restricted-actor', '受限要求', 'confirmed', 9000)"#,
        vec![],
    )
    .await;
    exec(db, "INSERT INTO secretary_thread_claim_sources (claim_id, source_event_id) VALUES ('thr009-visible-claim', ?), ('thr009-secret-claim', ?)", vec![visible_b.as_str().into(), restricted.as_str().into()]).await;
    exec(db, "INSERT INTO secretary_thread_decisions (decision_id, thread_id, statement, status, confidence_bps) VALUES ('thr009-secret-decision', 'thr009-thread-b', '受限结论', 'confirmed', 9500)", vec![]).await;
    exec(db, "INSERT INTO secretary_thread_decision_sources (decision_id, source_event_id) VALUES ('thr009-secret-decision', ?)", vec![restricted.as_str().into()]).await;
    exec(
        db,
        r#"INSERT INTO secretary_thread_open_questions
        (question_id, thread_id, raised_by_channel, raised_by_account, raised_by_actor_id,
         question, status, confidence_bps)
        VALUES ('thr009-secret-question', 'thr009-thread-b', 'napcat', 'thr009-managed',
                'thr009-restricted-actor', '受限问题', 'open', 9000)"#,
        vec![],
    )
    .await;
    exec(db, "INSERT INTO secretary_thread_question_sources (question_id, source_event_id) VALUES ('thr009-secret-question', ?)", vec![restricted.as_str().into()]).await;
    exec(db, r#"INSERT INTO secretary_response_expectations
        (expectation_id, account_id, source_question_id, thread_id, due_at_unix_secs)
        SELECT 'thr009-expectation', account_id, 'thr009-secret-question', 'thr009-thread-b', 1900001000
        FROM secretary_source_events WHERE source_event_id = ?"#, vec![restricted.as_str().into()]).await;

    let resolution_change = Uuid::new_v4().to_string();
    exec(db, r#"INSERT INTO secretary_thread_status_history
        (change_id, thread_id, from_status, to_status, authority, reason)
        VALUES (?, 'thr009-thread-b', 'open', 'resolved', 'evidence_derived', 'explicit_resolution')"#, vec![resolution_change.clone().into()]).await;
    exec(
        db,
        "INSERT INTO secretary_thread_status_sources (change_id, source_event_id) VALUES (?, ?)",
        vec![resolution_change.into(), restricted.as_str().into()],
    )
    .await;

    exec(db, "INSERT INTO secretary_thread_semantic_state (thread_id, lease_token, lease_expires_at, attempts) VALUES ('thr009-thread-b', 'thr009-semantic-lease', UTC_TIMESTAMP(6) + INTERVAL 60 SECOND, 1)", vec![]).await;
    exec(db, "INSERT INTO secretary_thread_link_scan_state (source_event_id, lease_token, lease_expires_at, attempts) VALUES (?, 'thr009-link-lease', UTC_TIMESTAMP(6) + INTERVAL 60 SECOND, 1)", vec![restricted.as_str().into()]).await;
    exec(
        db,
        r#"INSERT INTO secretary_memory_candidate_processing_state
        (account_id, lease_token, lease_expires_at, attempts)
        SELECT account_id, 'thr009-memory-lease', UTC_TIMESTAMP(6) + INTERVAL 60 SECOND, 1
        FROM secretary_source_events WHERE source_event_id = ?"#,
        vec![restricted.as_str().into()],
    )
    .await;

    exec(db, r#"INSERT INTO secretary_thread_link_candidates
        (candidate_id, account_id, left_thread_id, right_thread_id, left_conversation_id,
         right_conversation_id, signal_kind, fingerprint_sha256, status, confidence_bps, reason_code)
        SELECT 'thr009-link-candidate', left_event.account_id, 'thr009-thread-a', 'thr009-thread-b',
               left_event.conversation_id, right_event.conversation_id, 'exact_rich_content_key', ?,
               'proposed', 8500, 'exact_rich_content_key'
        FROM secretary_source_events left_event JOIN secretary_source_events right_event
        WHERE left_event.source_event_id = ? AND right_event.source_event_id = ?"#,
        vec!["9".repeat(64).into(), visible_a.as_str().into(), restricted.as_str().into()]).await;
    exec(db, "INSERT INTO secretary_thread_link_candidate_sources (candidate_id, source_event_id) VALUES ('thr009-link-candidate', ?), ('thr009-link-candidate', ?)", vec![visible_a.as_str().into(), restricted.as_str().into()]).await;

    exec(
        db,
        r#"INSERT INTO secretary_memory_candidates
        (candidate_id, account_id, candidate_kind, subject_key, payload_json,
         candidate_status, extractor_version, deterministic_fingerprint)
        SELECT 'thr009-memory-candidate', account_id, 'project', 'thr009-project', JSON_OBJECT(),
               'proposed', 'thr009', ? FROM secretary_source_events WHERE source_event_id = ?"#,
        vec!["8".repeat(64).into(), restricted.as_str().into()],
    )
    .await;
    exec(db, r#"INSERT INTO secretary_memory_candidate_sources
        (candidate_id, source_event_id, account_id, actor_platform_id,
         content_trust_level, occurred_at_unix_secs)
        SELECT 'thr009-memory-candidate', source_event_id, account_id, actor_platform_id,
               'normal', occurred_at_unix_secs FROM secretary_source_events WHERE source_event_id = ?"#,
        vec![restricted.as_str().into()]).await;
    exec(
        db,
        r#"INSERT INTO secretary_memory_facts
        (fact_id, account_id, fact_kind, subject_key, fact_json, fact_status, confidence_bps)
        SELECT 'thr009-memory-fact', account_id, 'commitment', 'thr009-commitment',
               JSON_OBJECT('restricted', TRUE), 'confirmed', 9000
        FROM secretary_source_events WHERE source_event_id = ?"#,
        vec![restricted.as_str().into()],
    )
    .await;
    exec(db, "INSERT INTO secretary_memory_fact_sources (fact_id, source_event_id) VALUES ('thr009-memory-fact', ?)", vec![restricted.as_str().into()]).await;
    exec(db, r#"INSERT INTO secretary_follow_up_items
        (follow_up_id, account_id, source_memory_fact_id, reason_code, due_at_unix_secs, status)
        SELECT 'thr009-follow-up', account_id, 'thr009-memory-fact', 'commitment_due', 1900001000, 'scheduled'
        FROM secretary_source_events WHERE source_event_id = ?"#, vec![restricted.as_str().into()]).await;

    exec(db, r#"INSERT INTO secretary_participant_profiles
        (account_id, platform_identity_kind, actor_platform_id, display_name, aliases_json,
         trust, source_event_ids_json, established_by_event_id)
        SELECT account_id, 'external', 'thr009-restricted-actor', '受限昵称', JSON_ARRAY(),
               'observed', JSON_ARRAY(?), ? FROM secretary_source_events WHERE source_event_id = ?"#,
        vec![restricted.as_str().into(), restricted.as_str().into(), restricted.as_str().into()]).await;
    exec(
        db,
        r#"INSERT INTO secretary_participant_conversation_observations
        (account_id, conversation_id, platform_identity_kind, actor_platform_id, group_card,
         group_role, established_by_event_id, source_event_ids_json)
        SELECT account_id, conversation_id, 'external', 'thr009-restricted-actor', '受限群名片',
               'member', source_event_id, JSON_ARRAY(source_event_id)
        FROM secretary_source_events WHERE source_event_id = ?"#,
        vec![restricted.as_str().into()],
    )
    .await;
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        sql,
        values,
    ))
    .await
    .unwrap_or_else(|error| panic!("THR-009 seed SQL failed: {error}"));
}

async fn assert_status(
    db: &DatabaseConnection,
    table: &str,
    id_column: &str,
    id: &str,
    status_column: &str,
    expected: &str,
) {
    let sql = format!("SELECT {status_column} AS value FROM {table} WHERE {id_column} = ?");
    assert_eq!(
        common::scalar_string(db, &sql, vec![id.into()]).await,
        expected
    );
}
