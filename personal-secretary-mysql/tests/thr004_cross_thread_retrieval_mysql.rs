//! THR-004 deterministic cross-thread retrieval against isolated MySQL.
//!
//! Requires QQBOT_TEST_DATABASE_URL pointing to an isolated `qqbot_accept_*` schema.

mod common;

use std::sync::Arc;

use personal_secretary::{
    ContentTrustLevel, RetrieverPolicy, RetrieverUseCase, SourceEventId, ThreadSearchMatchRank,
    VerifiedActorKind,
};
use personal_secretary_mysql::{build_mysql_inbound_event_store, build_mysql_retriever_store};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn cross_thread_search_is_ranked_private_effective_and_account_scoped() {
    let (db, schema) = common::isolated_db("_thr004").await;
    let scenario_db = db.clone();
    let result = tokio::spawn(async move { run_scenario(scenario_db).await }).await;
    common::drop_schema(&db, &schema).await;
    result.expect("THR-004 MySQL scenario must complete");
}

async fn run_scenario(db: DatabaseConnection) {
    let inbound = build_mysql_inbound_event_store(db.clone());
    let exact = common::insert_group_message(
        &inbound,
        "thr004-account-a",
        "exact",
        "group-exact",
        "actor-exact",
        VerifiedActorKind::External,
        100,
        "alpha",
    )
    .await;
    let prefix = common::insert_group_message(
        &inbound,
        "thr004-account-a",
        "prefix",
        "group-prefix",
        "actor-prefix",
        VerifiedActorKind::External,
        300,
        "alpha project update",
    )
    .await;
    let contains = common::insert_group_message(
        &inbound,
        "thr004-account-a",
        "contains",
        "group-contains",
        "actor-contains",
        VerifiedActorKind::External,
        500,
        "status for alpha project",
    )
    .await;
    let literal = common::insert_group_message(
        &inbound,
        "thr004-account-a",
        "literal",
        "group-literal",
        "actor-literal",
        VerifiedActorKind::External,
        600,
        "cost 100%_done",
    )
    .await;
    let wildcard_distractor = common::insert_group_message(
        &inbound,
        "thr004-account-a",
        "wildcard-distractor",
        "group-wildcard",
        "actor-wildcard",
        VerifiedActorKind::External,
        700,
        "cost 100xxdone",
    )
    .await;
    let local_only = common::insert_group_message(
        &inbound,
        "thr004-account-a",
        "local-only",
        "group-local",
        "actor-local",
        VerifiedActorKind::External,
        800,
        "alpha local secret",
    )
    .await;
    let envelope_only = common::insert_group_message(
        &inbound,
        "thr004-account-a",
        "envelope-only",
        "group-envelope",
        "actor-envelope",
        VerifiedActorKind::External,
        900,
        "alpha envelope secret",
    )
    .await;
    let other_account = common::insert_group_message(
        &inbound,
        "thr004-account-b",
        "other-account",
        "group-other",
        "actor-other",
        VerifiedActorKind::External,
        1_000,
        "alpha",
    )
    .await;
    let exact_recent_nonmatch = common::insert_group_message(
        &inbound,
        "thr004-account-a",
        "exact-recent-nonmatch",
        "group-exact",
        "actor-exact-2",
        VerifiedActorKind::External,
        250,
        "unrelated deployment note",
    )
    .await;
    let exact_tie_a = common::insert_group_message(
        &inbound,
        "thr004-account-a",
        "exact-tie-a",
        "group-tie-a",
        "actor-tie-a",
        VerifiedActorKind::External,
        200,
        "alpha",
    )
    .await;
    let exact_tie_z = common::insert_group_message(
        &inbound,
        "thr004-account-a",
        "exact-tie-z",
        "group-tie-z",
        "actor-tie-z",
        VerifiedActorKind::External,
        200,
        "alpha",
    )
    .await;

    for (thread, account, event) in [
        ("thr004-exact", "thr004-account-a", &exact),
        ("thr004-prefix", "thr004-account-a", &prefix),
        ("thr004-contains", "thr004-account-a", &contains),
        ("thr004-literal", "thr004-account-a", &literal),
        ("thr004-wildcard", "thr004-account-a", &wildcard_distractor),
        ("thr004-local", "thr004-account-a", &local_only),
        ("thr004-envelope", "thr004-account-a", &envelope_only),
        ("thr004-other", "thr004-account-b", &other_account),
        ("thr004-tie-a", "thr004-account-a", &exact_tie_a),
        ("thr004-tie-z", "thr004-account-a", &exact_tie_z),
    ] {
        create_thread(&db, thread, account, event, event).await;
        attach_event(&db, thread, event).await;
    }
    attach_event(&db, "thr004-exact", &exact_recent_nonmatch).await;
    set_content_mode(&db, &local_only, "local_only").await;
    set_content_mode(&db, &envelope_only, "envelope_only").await;

    let account = common::account("thr004-account-a");
    let retriever = Arc::new(RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    ));

    let ranked = retriever
        .search_threads(&account, "alpha", 20)
        .await
        .expect("ranked search");
    assert_eq!(ranked[0].thread_id.as_str(), "thr004-exact");
    assert_eq!(ranked[0].match_rank, ThreadSearchMatchRank::Exact);
    assert_eq!(ranked[1].thread_id.as_str(), "thr004-tie-a");
    assert_eq!(ranked[2].thread_id.as_str(), "thr004-tie-z");
    assert_eq!(ranked[3].thread_id.as_str(), "thr004-prefix");
    assert_eq!(ranked[3].match_rank, ThreadSearchMatchRank::Prefix);
    assert_eq!(ranked[4].thread_id.as_str(), "thr004-contains");
    assert_eq!(ranked[4].match_rank, ThreadSearchMatchRank::Contains);
    assert!(
        ranked
            .iter()
            .all(|result| result.thread_id.as_str() != "thr004-envelope")
    );
    assert!(
        ranked
            .iter()
            .all(|result| result.thread_id.as_str() != "thr004-other")
    );
    assert!(
        ranked
            .iter()
            .all(|result| result.thread_id.as_str() != "thr004-local")
    );
    assert_eq!(
        ranked[0].representative_source_event_id, exact,
        "representative event must be the actual matching source"
    );
    assert_eq!(ranked[0].representative_actor.id, "actor-exact");
    assert_eq!(ranked[0].representative_conversation.id, "group-exact");
    assert_eq!(ranked[0].representative_occurred_at_unix_secs, 100);
    assert_eq!(ranked[0].latest_event_at_unix_secs, 250);
    assert_eq!(ranked[0].event_count, 2);

    let mut cursor = None;
    let mut paged_ids = Vec::new();
    for expected_len in [2, 2, 1] {
        let page = retriever
            .search_threads_page(&account, "alpha", cursor.as_ref(), 2)
            .await
            .expect("thread search page");
        assert_eq!(page.threads.len(), expected_len);
        paged_ids.extend(
            page.threads
                .iter()
                .map(|thread| thread.thread_id.as_str().to_owned()),
        );
        cursor = page.next_cursor;
    }
    assert!(cursor.is_none());
    assert_eq!(
        paged_ids,
        ranked
            .iter()
            .map(|thread| thread.thread_id.as_str().to_owned())
            .collect::<Vec<_>>()
    );
    let mismatched_cursor = cursor_for_query(&retriever, &account).await;
    assert!(
        retriever
            .search_threads_page(&account, "different", Some(&mismatched_cursor), 2)
            .await
            .is_err(),
        "a thread cursor cannot be reused with another query"
    );

    let remote = retriever
        .search_threads_for_model(&account, "alpha", 20, false)
        .await
        .expect("remote-safe search");
    assert!(remote.iter().all(|result| {
        result.representative_content_trust_level == ContentTrustLevel::Normal
            && result.thread_id.as_str() != "thr004-local"
    }));
    let local_retriever = RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy {
            allow_local_only_to_loopback_llm: true,
        },
    );
    let loopback = local_retriever
        .search_threads_for_model(&account, "alpha", 20, true)
        .await
        .expect("loopback local-only search");
    assert!(
        loopback
            .iter()
            .any(|result| result.thread_id.as_str() == "thr004-local")
    );

    let literal_results = retriever
        .search_threads(&account, "%_", 20)
        .await
        .expect("literal wildcard search");
    assert_eq!(literal_results.len(), 1);
    assert_eq!(literal_results[0].thread_id.as_str(), "thr004-literal");

    verify_effective_merge_and_split(&db, &inbound, &retriever, &account).await;
    verify_pending_owner_work_paging(&db, &inbound, &retriever, &account).await;
}

async fn cursor_for_query(
    retriever: &RetrieverUseCase,
    account: &personal_secretary::SourceAccountRef,
) -> personal_secretary::ThreadSearchCursor {
    retriever
        .search_threads_page(account, "alpha", None, 1)
        .await
        .expect("cursor seed page")
        .next_cursor
        .expect("cursor seed must have a next page")
}

async fn verify_pending_owner_work_paging(
    db: &DatabaseConnection,
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    retriever: &RetrieverUseCase,
    account: &personal_secretary::SourceAccountRef,
) {
    let source_a = common::insert_group_message(
        inbound,
        "thr004-account-a",
        "pending-source-a",
        "pending-group-a",
        "pending-actor-a",
        VerifiedActorKind::Owner,
        2_000,
        "pending seed",
    )
    .await;
    let source_b = common::insert_group_message(
        inbound,
        "thr004-account-b",
        "pending-source-b",
        "pending-group-b",
        "pending-actor-b",
        VerifiedActorKind::Owner,
        2_000,
        "other account pending seed",
    )
    .await;
    for (suffix, platform_account, source, due_at) in [
        ("0001", "thr004-account-a", &source_a, Some(100_i64)),
        ("0002", "thr004-account-a", &source_a, Some(100_i64)),
        ("0003", "thr004-account-a", &source_a, Some(200_i64)),
        ("0004", "thr004-account-a", &source_a, None),
        ("0005", "thr004-account-a", &source_a, None),
        ("9999", "thr004-account-b", &source_b, Some(50_i64)),
    ] {
        let item_id = format!("00000000-0000-0000-0000-00000000{suffix}");
        let inserted = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO secretary_agenda_items \
                 (item_id, account_id, item_kind, title, scheduled_at_unix_secs, timezone_name, \
                  item_status, version, created_command_event_id, current_command_event_id, \
                  create_idempotency_key) \
                 SELECT ?, id, 'task', ?, ?, 'Asia/Shanghai', 'scheduled', 1, ?, ?, ? \
                 FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
                [
                    item_id.clone().into(),
                    format!("pending-{suffix}").into(),
                    due_at.into(),
                    source.as_str().into(),
                    source.as_str().into(),
                    format!("pending-key-{suffix}").into(),
                    platform_account.into(),
                ],
            ))
            .await
            .expect("insert pending agenda item");
        assert_eq!(inserted.rows_affected(), 1);
    }

    let mut cursor = None;
    let mut collected = Vec::new();
    for expected_len in [2, 2, 1] {
        let page = retriever
            .list_pending_owner_work_page(account, cursor.as_ref(), 2)
            .await
            .expect("pending owner work page");
        assert_eq!(page.items.len(), expected_len);
        collected.extend(page.items.iter().map(|item| {
            (
                item.source_id.clone(),
                item.due_at_unix_secs,
                item.source_kind.clone(),
            )
        }));
        cursor = page.next_cursor;
    }
    assert!(cursor.is_none());
    assert_eq!(
        collected
            .iter()
            .map(|(id, due, _)| (id.as_str(), *due))
            .collect::<Vec<_>>(),
        vec![
            ("00000000-0000-0000-0000-000000000001", Some(100)),
            ("00000000-0000-0000-0000-000000000002", Some(100)),
            ("00000000-0000-0000-0000-000000000003", Some(200)),
            ("00000000-0000-0000-0000-000000000004", None),
            ("00000000-0000-0000-0000-000000000005", None),
        ]
    );
    assert!(collected.iter().all(|(_, _, kind)| kind == "agenda"));
}

async fn verify_effective_merge_and_split(
    db: &DatabaseConnection,
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    retriever: &RetrieverUseCase,
    account: &personal_secretary::SourceAccountRef,
) {
    let merged_event = common::insert_group_message(
        inbound,
        "thr004-account-a",
        "merged-event",
        "group-merged",
        "actor-merged",
        VerifiedActorKind::External,
        1_100,
        "merged alpha evidence",
    )
    .await;
    let canonical_root = common::insert_group_message(
        inbound,
        "thr004-account-a",
        "canonical-root",
        "group-canonical",
        "actor-canonical",
        VerifiedActorKind::External,
        1_050,
        "canonical root",
    )
    .await;
    create_thread(
        db,
        "thr004-merged-old",
        "thr004-account-a",
        &merged_event,
        &merged_event,
    )
    .await;
    attach_event(db, "thr004-merged-old", &merged_event).await;
    create_thread(
        db,
        "thr004-canonical",
        "thr004-account-a",
        &canonical_root,
        &canonical_root,
    )
    .await;
    attach_event(db, "thr004-canonical", &canonical_root).await;
    let merge_proposal = insert_mutation_proposal(db, "merge").await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_merge_aliases \
         (merged_thread_id, canonical_thread_id, proposal_id, active) VALUES (?, ?, ?, TRUE)",
        [
            "thr004-merged-old".into(),
            "thr004-canonical".into(),
            merge_proposal.into(),
        ],
    ))
    .await
    .expect("insert merge alias");
    let merged = retriever
        .search_threads(account, "merged alpha", 10)
        .await
        .expect("effective merge search");
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].thread_id.as_str(), "thr004-canonical");

    let split_event = common::insert_group_message(
        inbound,
        "thr004-account-a",
        "split-event",
        "group-split",
        "actor-split",
        VerifiedActorKind::External,
        1_200,
        "split alpha evidence",
    )
    .await;
    let split_root = common::insert_group_message(
        inbound,
        "thr004-account-a",
        "split-root",
        "group-split-target",
        "actor-split-root",
        VerifiedActorKind::External,
        1_150,
        "split root",
    )
    .await;
    create_thread(
        db,
        "thr004-split-original",
        "thr004-account-a",
        &split_event,
        &split_event,
    )
    .await;
    attach_event(db, "thr004-split-original", &split_event).await;
    create_thread(
        db,
        "thr004-split-target",
        "thr004-account-a",
        &split_root,
        &split_root,
    )
    .await;
    attach_event(db, "thr004-split-target", &split_root).await;
    let split_proposal = insert_mutation_proposal(db, "split").await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_split_overrides \
         (source_event_id, original_thread_id, effective_thread_id, proposal_id, active) \
         VALUES (?, ?, ?, ?, TRUE)",
        [
            split_event.as_str().into(),
            "thr004-split-original".into(),
            "thr004-split-target".into(),
            split_proposal.into(),
        ],
    ))
    .await
    .expect("insert split override");
    let split = retriever
        .search_threads(account, "split alpha", 10)
        .await
        .expect("effective split search");
    assert_eq!(split.len(), 1);
    assert_eq!(split[0].thread_id.as_str(), "thr004-split-target");
}

async fn create_thread(
    db: &DatabaseConnection,
    thread_id: &str,
    account: &str,
    root: &SourceEventId,
    latest: &SourceEventId,
) {
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_event_threads \
             (thread_id, account_id, root_event_id, latest_event_id, \
              opened_at_unix_secs, latest_occurred_at_unix_secs) \
             SELECT ?, id, ?, ?, 100, 100 FROM secretary_accounts \
             WHERE source_channel = 'napcat' AND platform_account_id = ?",
            [
                thread_id.into(),
                root.as_str().into(),
                latest.as_str().into(),
                account.into(),
            ],
        ))
        .await
        .expect("create thread");
    assert_eq!(inserted.rows_affected(), 1);
}

async fn attach_event(db: &DatabaseConnection, thread_id: &str, event: &SourceEventId) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_events (source_event_id, thread_id) VALUES (?, ?)",
        [event.as_str().into(), thread_id.into()],
    ))
    .await
    .expect("attach event");
}

async fn set_content_mode(db: &DatabaseConnection, event: &SourceEventId, mode: &str) {
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_message_contents SET content_mode = ? WHERE source_event_id = ?",
            [mode.into(), event.as_str().into()],
        ))
        .await
        .expect("set content mode");
    assert_eq!(updated.rows_affected(), 1);
}

async fn insert_mutation_proposal(db: &DatabaseConnection, kind: &str) -> String {
    let proposal_id = Uuid::new_v4().to_string();
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_thread_mutation_proposals \
             (proposal_id, account_id, mutation_kind, impact_json) \
             SELECT ?, id, ?, '{}' FROM secretary_accounts \
             WHERE source_channel = 'napcat' AND platform_account_id = 'thr004-account-a'",
            [proposal_id.clone().into(), kind.into()],
        ))
        .await
        .expect("insert mutation proposal");
    assert_eq!(inserted.rows_affected(), 1);
    proposal_id
}
