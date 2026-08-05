//! THR-005 immutable decision revision keyset paging against isolated MySQL.
//!
//! Requires QQBOT_TEST_DATABASE_URL pointing to an isolated `qqbot_accept_*` schema.

mod common;

use personal_secretary::{
    EventThreadId, RetrieverPolicy, RetrieverUseCase, ThreadDecisionId, VerifiedActorKind,
};
use personal_secretary_mysql::{build_mysql_inbound_event_store, build_mysql_retriever_store};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

const MIGRATION_NAME: &str = "20260806_qqbot_thread_decision_revision_paging.sql";

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn decision_revisions_page_stably_without_rewriting_history() {
    let (db, schema) = common::isolated_db("_thr005").await;
    let scenario_db = db.clone();
    let result = tokio::spawn(async move { run_scenario(scenario_db).await }).await;
    common::drop_schema(&db, &schema).await;
    result.expect("THR-005 MySQL scenario must complete");
}

async fn run_scenario(db: DatabaseConnection) {
    let inbound = build_mysql_inbound_event_store(db.clone());
    let mut source_events = Vec::new();
    for sequence in 1..=6 {
        source_events.push(
            common::insert_group_message(
                &inbound,
                "thr005-account-a",
                &format!("decision-source-{sequence}"),
                "thr005-group-a",
                "thr005-actor-a",
                VerifiedActorKind::External,
                1_800_000_000 + sequence,
                &format!("decision evidence {sequence}"),
            )
            .await,
        );
    }
    let account_b_event = common::insert_group_message(
        &inbound,
        "thr005-account-b",
        "decision-source-b",
        "thr005-group-b",
        "thr005-actor-b",
        VerifiedActorKind::External,
        1_800_000_100,
        "other account evidence",
    )
    .await;

    create_thread(
        &db,
        "thr005-thread-a",
        "thr005-account-a",
        source_events[0].as_str(),
    )
    .await;
    create_thread(
        &db,
        "thr005-thread-b",
        "thr005-account-a",
        source_events[1].as_str(),
    )
    .await;
    create_thread(
        &db,
        "thr005-thread-other-account",
        "thr005-account-b",
        account_b_event.as_str(),
    )
    .await;

    let decision_ids = [
        "00000000-0000-0000-0000-000000000001",
        "00000000-0000-0000-0000-000000000002",
        "00000000-0000-0000-0000-000000000003",
        "00000000-0000-0000-0000-000000000004",
        "00000000-0000-0000-0000-000000000005",
        "00000000-0000-0000-0000-000000000006",
    ];
    let created_at = [
        "2026-08-06 01:00:00.100000",
        "2026-08-06 01:00:00.200000",
        "2026-08-06 01:00:00.300000",
        "2026-08-06 01:00:00.400000",
        "2026-08-06 01:00:00.400000",
        "2026-08-06 01:00:00.500000",
    ];
    for index in 0..decision_ids.len() {
        insert_decision(
            &db,
            decision_ids[index],
            "thr005-thread-a",
            &format!("immutable revision {}", index + 1),
            if index + 1 == decision_ids.len() {
                "confirmed"
            } else {
                "superseded"
            },
            8_000 + u32::try_from(index).unwrap(),
            index.checked_sub(1).map(|previous| decision_ids[previous]),
            created_at[index],
            source_events[index].as_str(),
        )
        .await;
    }
    insert_decision(
        &db,
        "10000000-0000-0000-0000-000000000001",
        "thr005-thread-other-account",
        "other account revision",
        "confirmed",
        9_000,
        None,
        "2026-08-06 01:00:01.000000",
        account_b_event.as_str(),
    )
    .await;

    let before = decision_snapshot(&db).await;
    let retriever = RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    );
    let account_a = common::account("thr005-account-a");
    let thread_a = EventThreadId::new("thr005-thread-a").unwrap();

    let page_1 = retriever
        .thread_decision_revisions(&account_a, &thread_a, None, 2)
        .await
        .expect("first revision page");
    assert_eq!(
        decision_names(&page_1.decisions),
        decision_ids[4..6].iter().rev().copied().collect::<Vec<_>>()
    );
    assert_eq!(page_1.decisions[0].statement, "immutable revision 6");
    assert_eq!(page_1.decisions[0].status, "confirmed");
    assert_eq!(page_1.decisions[1].status, "superseded");
    assert_eq!(page_1.decisions[0].confidence_bps, 8_005);
    assert_eq!(
        page_1.decisions[0].source_event_ids,
        vec![source_events[5].clone()]
    );
    let cursor_1 = page_1.next_cursor.as_ref().expect("first continuation");
    assert_eq!(cursor_1.thread_id(), &thread_a);
    assert_eq!(cursor_1.decision_id().as_str(), decision_ids[4]);

    let replayed_page_1 = retriever
        .thread_decision_revisions(&account_a, &thread_a, None, 2)
        .await
        .expect("deterministic first page replay");
    assert_eq!(replayed_page_1, page_1);

    let page_2 = retriever
        .thread_decision_revisions(&account_a, &thread_a, Some(cursor_1), 2)
        .await
        .expect("second revision page");
    assert_eq!(
        decision_names(&page_2.decisions),
        vec![decision_ids[3], decision_ids[2]]
    );
    let cursor_2 = page_2.next_cursor.as_ref().expect("second continuation");

    let page_3 = retriever
        .thread_decision_revisions(&account_a, &thread_a, Some(cursor_2), 2)
        .await
        .expect("third revision page");
    assert_eq!(
        decision_names(&page_3.decisions),
        vec![decision_ids[1], decision_ids[0]]
    );
    assert!(page_3.next_cursor.is_none());

    let mut all_decisions = Vec::new();
    all_decisions.extend(page_1.decisions.clone());
    all_decisions.extend(page_2.decisions.clone());
    all_decisions.extend(page_3.decisions.clone());
    assert_eq!(all_decisions.len(), 6);
    for (descending_index, decision) in all_decisions.iter().enumerate() {
        let original_index = 5 - descending_index;
        assert_eq!(decision.decision_id.as_str(), decision_ids[original_index]);
        assert_eq!(
            decision.supersedes.as_ref().map(ThreadDecisionId::as_str),
            original_index
                .checked_sub(1)
                .map(|previous| decision_ids[previous])
        );
    }

    let cross_account = retriever
        .thread_decision_revisions(&common::account("thr005-account-b"), &thread_a, None, 10)
        .await
        .expect("cross-account query is empty");
    assert!(cross_account.decisions.is_empty());
    assert!(cross_account.next_cursor.is_none());

    let cross_thread = retriever
        .thread_decision_revisions(
            &account_a,
            &EventThreadId::new("thr005-thread-b").unwrap(),
            page_1.next_cursor.as_ref(),
            2,
        )
        .await;
    assert!(
        cross_thread.is_err(),
        "cross-thread cursor must fail closed"
    );

    assert_eq!(decision_snapshot(&db).await, before);
    verify_revision_index(&db).await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM qqbot_test_schema_migrations WHERE migration_name = ?",
        [MIGRATION_NAME.into()],
    ))
    .await
    .expect("remove THR-005 migration record for replay");
    common::try_apply_qqbot_migrations(&db)
        .await
        .expect("THR-005 index rebuild must be safely replayable");
    verify_revision_index(&db).await;
    assert_eq!(decision_snapshot(&db).await, before);
}

fn decision_names(decisions: &[personal_secretary::ThreadDecisionSummary]) -> Vec<&str> {
    decisions
        .iter()
        .map(|decision| decision.decision_id.as_str())
        .collect()
}

async fn create_thread(
    db: &DatabaseConnection,
    thread_id: &str,
    account: &str,
    root_event_id: &str,
) {
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_event_threads \
             (thread_id, account_id, root_event_id, latest_event_id, \
              opened_at_unix_secs, latest_occurred_at_unix_secs) \
             SELECT ?, id, ?, ?, 1800000000, 1800000000 FROM secretary_accounts \
             WHERE source_channel = 'napcat' AND platform_account_id = ?",
            [
                thread_id.into(),
                root_event_id.into(),
                root_event_id.into(),
                account.into(),
            ],
        ))
        .await
        .expect("create THR-005 thread");
    assert_eq!(inserted.rows_affected(), 1);
}

#[allow(clippy::too_many_arguments)]
async fn insert_decision(
    db: &DatabaseConnection,
    decision_id: &str,
    thread_id: &str,
    statement: &str,
    status: &str,
    confidence_bps: u32,
    supersedes_id: Option<&str>,
    created_at: &str,
    source_event_id: &str,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_decisions \
         (decision_id, thread_id, statement, status, confidence_bps, supersedes_id, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        vec![
            decision_id.into(),
            thread_id.into(),
            statement.into(),
            status.into(),
            confidence_bps.into(),
            supersedes_id.map_or(sea_orm::Value::String(None), |value| value.into()),
            created_at.into(),
        ],
    ))
    .await
    .expect("insert immutable decision revision");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_decision_sources (decision_id, source_event_id) VALUES (?, ?)",
        [decision_id.into(), source_event_id.into()],
    ))
    .await
    .expect("insert decision source");
}

async fn decision_snapshot(db: &DatabaseConnection) -> String {
    common::scalar_string(
        db,
        "SELECT GROUP_CONCAT(HEX(CONCAT_WS('|', decision_id, statement, status, \
         confidence_bps, COALESCE(supersedes_id, ''), DATE_FORMAT(created_at, '%Y-%m-%d %H:%i:%s.%f'), \
         DATE_FORMAT(updated_at, '%Y-%m-%d %H:%i:%s.%f'))) \
         ORDER BY decision_id SEPARATOR ',') AS value \
         FROM secretary_thread_decisions WHERE thread_id = 'thr005-thread-a'",
        Vec::new(),
    )
    .await
}

async fn verify_revision_index(db: &DatabaseConnection) {
    let count = common::scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM ( \
           SELECT INDEX_NAME, MIN(NON_UNIQUE) AS non_unique, \
                  GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ',') AS columns_found \
           FROM INFORMATION_SCHEMA.STATISTICS \
           WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'secretary_thread_decisions' \
             AND INDEX_NAME = 'idx_secretary_thread_decision_thread' \
           GROUP BY INDEX_NAME \
         ) indexes_found WHERE non_unique = 1 \
           AND columns_found = 'thread_id,created_at,decision_id,status'",
        Vec::new(),
    )
    .await;
    assert_eq!(count, 1, "revision paging index must have the exact shape");
}
