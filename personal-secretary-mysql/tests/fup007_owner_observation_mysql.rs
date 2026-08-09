//! FUP-007：NapCat Owner 后续消息必须结束私聊回复期待，群聊保持严格作用域。

mod common;

use personal_secretary_mysql::build_mysql_follow_up_store;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn owner_observation_resolves_only_scoped_response_expectations() {
    let (db, schema) = common::isolated_db("_fup007_owner_reply").await;
    let scenario_db = db.clone();
    let result = tokio::spawn(async move { run_scenario(scenario_db).await }).await;
    common::drop_schema(&db, &schema).await;
    result.expect("FUP-007 Owner observation scenario must complete");
}

async fn run_scenario(db: DatabaseConnection) {
    exec(
        &db,
        "INSERT INTO secretary_accounts (source_channel, platform_account_id, status) \
         VALUES ('napcat', 'fup007-owner-observation', 'active')",
    )
    .await;
    let account_id = common::scalar_u64(
        &db,
        "SELECT id AS value FROM secretary_accounts \
         WHERE source_channel = 'napcat' AND platform_account_id = 'fup007-owner-observation'",
        vec![],
    )
    .await;

    for (kind, platform_id) in [("private", "private-peer"), ("group", "group-scope")] {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_conversations \
                (account_id, conversation_kind, platform_conversation_id) VALUES (?, ?, ?)",
            vec![account_id.into(), kind.into(), platform_id.into()],
        ))
        .await
        .expect("seed conversation");
    }
    let private_conversation = conversation_id(&db, account_id, "private").await;
    let group_conversation = conversation_id(&db, account_id, "group").await;

    seed_event(
        &db,
        account_id,
        private_conversation,
        "private-asked",
        "private-asked-platform",
        "external",
        "external_observation",
        1_000,
        None,
    )
    .await;
    seed_event(
        &db,
        account_id,
        private_conversation,
        "private-owner-reply",
        "private-owner-reply-platform",
        "owner",
        "owner_observation",
        1_010,
        None,
    )
    .await;
    seed_event(
        &db,
        account_id,
        group_conversation,
        "group-asked",
        "group-asked-platform",
        "external",
        "external_observation",
        2_000,
        None,
    )
    .await;
    seed_event(
        &db,
        account_id,
        group_conversation,
        "group-owner-unrelated",
        "group-owner-unrelated-platform",
        "owner",
        "owner_observation",
        2_010,
        None,
    )
    .await;
    seed_event(
        &db,
        account_id,
        group_conversation,
        "group-explicit-asked",
        "group-explicit-asked-platform",
        "external",
        "external_observation",
        3_000,
        None,
    )
    .await;
    seed_event(
        &db,
        account_id,
        group_conversation,
        "group-explicit-reply",
        "group-explicit-reply-platform",
        "owner",
        "owner_observation",
        3_010,
        Some("group-explicit-asked"),
    )
    .await;

    for (thread_id, event_id, occurred_at) in [
        ("private-question-thread", "private-asked", 1_000),
        ("private-owner-thread", "private-owner-reply", 1_010),
        ("group-question-thread", "group-asked", 2_000),
        ("group-owner-thread", "group-owner-unrelated", 2_010),
        ("group-explicit-question", "group-explicit-asked", 3_000),
        ("group-explicit-owner", "group-explicit-reply", 3_010),
    ] {
        seed_thread(&db, account_id, thread_id, event_id, occurred_at).await;
    }

    seed_question(
        &db,
        "private-question",
        "private-question-thread",
        "private-asked",
    )
    .await;
    seed_question(
        &db,
        "group-question",
        "group-question-thread",
        "group-asked",
    )
    .await;
    seed_question(
        &db,
        "group-explicit-question-id",
        "group-explicit-question",
        "group-explicit-asked",
    )
    .await;

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_response_expectations \
            (expectation_id, account_id, source_question_id, thread_id, \
             source_version, due_at_unix_secs, expectation_status) \
         VALUES ('private-expectation', ?, 'private-question', \
                 'private-question-thread', 1, 1300, 'active')",
        [account_id.into()],
    ))
    .await
    .expect("seed active private expectation");

    let report = build_mysql_follow_up_store(db.clone())
        .scan_response_expectations(10_000, 3_600, 300, 100)
        .await
        .expect("scan response expectations");

    assert_eq!(report.response_expectations_resolved, 1);
    assert_eq!(report.response_expectations_materialized, 1);
    assert_eq!(
        expectation_status(&db, "private-question").await,
        "resolved"
    );
    assert_eq!(expectation_version(&db, "private-question").await, 2);
    assert_eq!(expectation_status(&db, "group-question").await, "active");
    assert_eq!(
        expectation_count(&db, "group-explicit-question-id").await,
        0
    );
}

#[allow(clippy::too_many_arguments)]
async fn seed_event(
    db: &DatabaseConnection,
    account_id: u64,
    conversation_id: u64,
    source_event_id: &str,
    platform_event_id: &str,
    actor_kind: &str,
    message_role: &str,
    occurred_at: i64,
    reply_to_event_id: Option<&str>,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_source_events \
            (source_event_id, account_id, conversation_id, source_channel, platform_event_id, \
             event_type, actor_platform_id, actor_kind, message_role, occurred_at_unix_secs, \
             reply_to_event_id, processing_status, received_at) \
         VALUES (?, ?, ?, 'napcat', ?, 'message', ?, ?, ?, ?, ?, 'processed', \
                 FROM_UNIXTIME(?))",
        vec![
            source_event_id.into(),
            account_id.into(),
            conversation_id.into(),
            platform_event_id.into(),
            format!("actor-{actor_kind}").into(),
            actor_kind.into(),
            message_role.into(),
            occurred_at.into(),
            reply_to_event_id.map(str::to_owned).into(),
            occurred_at.into(),
        ],
    ))
    .await
    .expect("seed source event");
}

async fn seed_thread(
    db: &DatabaseConnection,
    account_id: u64,
    thread_id: &str,
    event_id: &str,
    occurred_at: i64,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_event_threads \
            (thread_id, account_id, status, root_event_id, latest_event_id, \
             opened_at_unix_secs, latest_occurred_at_unix_secs) \
         VALUES (?, ?, 'open', ?, ?, ?, ?)",
        vec![
            thread_id.into(),
            account_id.into(),
            event_id.into(),
            event_id.into(),
            occurred_at.into(),
            occurred_at.into(),
        ],
    ))
    .await
    .expect("seed thread");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_events (source_event_id, thread_id) VALUES (?, ?)",
        [event_id.into(), thread_id.into()],
    ))
    .await
    .expect("seed thread member");
}

async fn seed_question(
    db: &DatabaseConnection,
    question_id: &str,
    thread_id: &str,
    source_event_id: &str,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_open_questions \
            (question_id, thread_id, raised_by_channel, raised_by_account, raised_by_actor_id, \
             question, status, confidence_bps) \
         VALUES (?, ?, 'napcat', 'managed', 'external-actor', \
                 'question fixture', 'open', 10000)",
        [question_id.into(), thread_id.into()],
    ))
    .await
    .expect("seed open question");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_question_sources (question_id, source_event_id) VALUES (?, ?)",
        [question_id.into(), source_event_id.into()],
    ))
    .await
    .expect("seed question source");
}

async fn conversation_id(db: &DatabaseConnection, account_id: u64, kind: &str) -> u64 {
    common::scalar_u64(
        db,
        "SELECT id AS value FROM secretary_conversations \
         WHERE account_id = ? AND conversation_kind = ?",
        vec![account_id.into(), kind.into()],
    )
    .await
}

async fn expectation_status(db: &DatabaseConnection, question_id: &str) -> String {
    common::scalar_string(
        db,
        "SELECT expectation_status AS value FROM secretary_response_expectations \
         WHERE source_question_id = ?",
        vec![question_id.into()],
    )
    .await
}

async fn expectation_version(db: &DatabaseConnection, question_id: &str) -> u64 {
    common::scalar_u64(
        db,
        "SELECT source_version AS value FROM secretary_response_expectations \
         WHERE source_question_id = ?",
        vec![question_id.into()],
    )
    .await
}

async fn expectation_count(db: &DatabaseConnection, question_id: &str) -> u64 {
    common::scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_response_expectations \
         WHERE source_question_id = ?",
        vec![question_id.into()],
    )
    .await
}

async fn exec(db: &DatabaseConnection, sql: &str) {
    db.execute_unprepared(sql)
        .await
        .expect("execute fixture SQL");
}
