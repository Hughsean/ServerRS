//! THR-006 source-backed automatic resolution against isolated MySQL.
//!
//! Requires QQBOT_TEST_DATABASE_URL pointing to an isolated `qqbot_accept_*` schema.

mod common;

use personal_secretary::{
    ConservativeThreadSemanticExtractor, SourceEventId, ThreadSemanticUseCase, VerifiedActorKind,
};
use personal_secretary_mysql::{
    build_mysql_inbound_event_store, build_mysql_thread_semantic_store,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn only_explicit_source_evidence_resolves_a_thread() {
    let (db, schema) = common::isolated_db("_thr006").await;
    let scenario_db = db.clone();
    let result = tokio::spawn(async move { run_scenario(scenario_db).await }).await;
    common::drop_schema(&db, &schema).await;
    result.expect("THR-006 MySQL scenario must complete");
}

async fn run_scenario(db: DatabaseConnection) {
    let inbound = build_mysql_inbound_event_store(db.clone());
    let use_case = ThreadSemanticUseCase::new(
        build_mysql_thread_semantic_store(db.clone()),
        Arc::new(ConservativeThreadSemanticExtractor::new(2_000).unwrap()),
        50,
        20_000,
        60,
    )
    .unwrap();

    let completion = insert_message(&inbound, "completion", "已完成：报价已发送", 1).await;
    create_thread(&db, "thr006-explicit", &completion).await;
    let completed = use_case
        .run_once()
        .await
        .expect("process explicit completion")
        .expect("explicit thread is claimable");
    assert!(completed.lifecycle_changed);
    assert_eq!(thread_status(&db, "thr006-explicit").await, "resolved");
    assert_eq!(history_count(&db, "thr006-explicit").await, 1);
    assert_eq!(
        common::scalar_string(
            &db,
            "SELECT CONCAT(authority, '|', reason, '|', from_status, '|', to_status) AS value \
             FROM secretary_thread_status_history WHERE thread_id = ?",
            vec!["thr006-explicit".into()],
        )
        .await,
        "evidence_derived|explicit_completion_evidence|open|resolved"
    );
    assert_eq!(
        common::scalar_string(
            &db,
            "SELECT source.source_event_id AS value \
             FROM secretary_thread_status_sources source \
             JOIN secretary_thread_status_history history ON history.change_id = source.change_id \
             WHERE history.thread_id = ?",
            vec!["thr006-explicit".into()],
        )
        .await,
        completion.as_str()
    );

    let silence = insert_message(&inbound, "silence", "三天没人发言，应该已经解决了", 2).await;
    create_thread(&db, "thr006-silence", &silence).await;
    let silent_run = use_case
        .run_once()
        .await
        .expect("process ambiguous silence")
        .expect("silence thread is claimable");
    assert!(!silent_run.lifecycle_changed);
    assert_eq!(thread_status(&db, "thr006-silence").await, "open");
    assert_eq!(history_count(&db, "thr006-silence").await, 0);

    let unresolved_question =
        insert_message(&inbound, "open-question", "问题已解决：服务恢复", 3).await;
    create_thread(&db, "thr006-open-question", &unresolved_question).await;
    insert_open_question(&db, "thr006-open-question", &unresolved_question).await;
    let question_run = use_case
        .run_once()
        .await
        .expect("process completion with open question")
        .expect("open-question thread is claimable");
    assert!(!question_run.lifecycle_changed);
    assert_eq!(thread_status(&db, "thr006-open-question").await, "open");
    assert_eq!(history_count(&db, "thr006-open-question").await, 0);
}

async fn insert_message(
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    message_id: &str,
    text: &str,
    sequence: i64,
) -> SourceEventId {
    common::insert_group_message(
        inbound,
        "thr006-account",
        message_id,
        "thr006-group",
        "thr006-actor",
        VerifiedActorKind::External,
        1_800_100_000 + sequence,
        text,
    )
    .await
}

async fn create_thread(db: &DatabaseConnection, thread_id: &str, event: &SourceEventId) {
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_event_threads \
             (thread_id, account_id, root_event_id, latest_event_id, \
              opened_at_unix_secs, latest_occurred_at_unix_secs) \
             SELECT ?, id, ?, ?, 1800100000, 1800100000 FROM secretary_accounts \
             WHERE source_channel = 'napcat' AND platform_account_id = 'thr006-account'",
            [
                thread_id.into(),
                event.as_str().into(),
                event.as_str().into(),
            ],
        ))
        .await
        .expect("create THR-006 thread");
    assert_eq!(inserted.rows_affected(), 1);
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_events (source_event_id, thread_id) VALUES (?, ?)",
        [event.as_str().into(), thread_id.into()],
    ))
    .await
    .expect("attach THR-006 source event");
}

async fn insert_open_question(
    db: &DatabaseConnection,
    thread_id: &str,
    source_event_id: &SourceEventId,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_open_questions \
         (question_id, thread_id, raised_by_channel, raised_by_account, raised_by_actor_id, \
          question, status, confidence_bps) \
         VALUES ('thr006-open-question-id', ?, 'napcat', 'thr006-account', 'thr006-actor', \
                 '仍需确认回归结果', 'open', 9000)",
        [thread_id.into()],
    ))
    .await
    .expect("insert open question");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_question_sources (question_id, source_event_id) \
         VALUES ('thr006-open-question-id', ?)",
        [source_event_id.as_str().into()],
    ))
    .await
    .expect("insert open-question source");
}

async fn thread_status(db: &DatabaseConnection, thread_id: &str) -> String {
    common::scalar_string(
        db,
        "SELECT status AS value FROM secretary_event_threads WHERE thread_id = ?",
        vec![thread_id.into()],
    )
    .await
}

async fn history_count(db: &DatabaseConnection, thread_id: &str) -> u64 {
    common::scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_thread_status_history WHERE thread_id = ?",
        vec![thread_id.into()],
    )
    .await
}
