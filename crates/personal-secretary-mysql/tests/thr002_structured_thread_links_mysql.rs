//! THR-002 structured cross-conversation link evidence against isolated MySQL.
//!
//! Requires QQBOT_TEST_DATABASE_URL pointing to an isolated `qqbot_accept_*` schema.

mod common;

use std::sync::Arc;

use personal_secretary::{
    ContentSegment, ConversationKind, ConversationRef, InboundMessageEnvelope, MediaKind,
    MessageSource, RichContentKind, SourceEventId, SourceMessageRef, ThreadLinkUseCase,
    VerifiedActor, VerifiedActorKind,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

const MIGRATION_NAME: &str = "20260806_qqbot_thread_link_structured_references.sql";

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn structured_references_create_only_account_scoped_proposed_candidates() {
    let (db, schema) = common::isolated_db("_thr002").await;
    let scenario_db = db.clone();
    let result = tokio::spawn(async move { run_scenario(scenario_db).await }).await;
    common::drop_schema(&db, &schema).await;
    result.expect("THR-002 MySQL scenario must complete");
}

async fn run_scenario(db: DatabaseConnection) {
    let inbound = personal_secretary_mysql::build_mysql_inbound_event_store(db.clone());

    let forward_a = insert_message(
        &inbound,
        "account-a",
        "forward-a",
        "group-forward-a",
        "actor-shared",
        "同一个话题",
        vec![ContentSegment::Forward {
            source_key: "Forward-Case-Sensitive".into(),
        }],
    )
    .await;
    let forward_b = insert_message(
        &inbound,
        "account-a",
        "forward-b",
        "group-forward-b",
        "actor-shared",
        "同一个话题",
        vec![ContentSegment::Forward {
            source_key: "Forward-Case-Sensitive".into(),
        }],
    )
    .await;

    let rich_key = format!("sha256:{}", "a".repeat(64));
    let rich_a = insert_message(
        &inbound,
        "account-a",
        "rich-a",
        "group-rich-a",
        "actor-rich",
        "卡片",
        vec![ContentSegment::Rich {
            kind: RichContentKind::Json,
            source_key: rich_key.clone(),
            summary: None,
        }],
    )
    .await;
    let rich_b = insert_message(
        &inbound,
        "account-a",
        "rich-b",
        "group-rich-b",
        "actor-rich",
        "卡片",
        vec![ContentSegment::Rich {
            kind: RichContentKind::Json,
            source_key: rich_key,
            summary: None,
        }],
    )
    .await;

    let file_v1 = insert_message(
        &inbound,
        "account-a",
        "file-v1",
        "group-file-v1",
        "actor-file",
        "报价单",
        vec![ContentSegment::Media {
            kind: MediaKind::File,
            source_key: "opaque-file-v1".into(),
            source_url: None,
            display_name: Some("报价单.pdf".into()),
        }],
    )
    .await;
    let file_v2 = insert_message(
        &inbound,
        "account-a",
        "file-v2",
        "group-file-v2",
        "actor-file",
        "报价单",
        vec![ContentSegment::FileVersionReference {
            current_source_key: "opaque-file-v2".into(),
            previous_source_key: "opaque-file-v1".into(),
        }],
    )
    .await;

    let weak_a = insert_message(
        &inbound,
        "account-a",
        "weak-a",
        "group-weak-a",
        "same-actor",
        "相似话题和同名文件",
        vec![ContentSegment::Media {
            kind: MediaKind::File,
            source_key: "different-file-a".into(),
            source_url: None,
            display_name: Some("同名文件.pdf".into()),
        }],
    )
    .await;
    let weak_b = insert_message(
        &inbound,
        "account-a",
        "weak-b",
        "group-weak-b",
        "same-actor",
        "相似话题和同名文件",
        vec![ContentSegment::Media {
            kind: MediaKind::File,
            source_key: "different-file-b".into(),
            source_url: None,
            display_name: Some("同名文件.pdf".into()),
        }],
    )
    .await;

    let other_account = insert_message(
        &inbound,
        "account-b",
        "forward-other-account",
        "group-forward-other-account",
        "actor-shared",
        "同一个话题",
        vec![ContentSegment::Forward {
            source_key: "Forward-Case-Sensitive".into(),
        }],
    )
    .await;

    for (account, event) in [
        ("account-a", &forward_a),
        ("account-a", &forward_b),
        ("account-a", &rich_a),
        ("account-a", &rich_b),
        ("account-a", &file_v1),
        ("account-a", &file_v2),
        ("account-a", &weak_a),
        ("account-a", &weak_b),
        ("account-b", &other_account),
    ] {
        attach_to_distinct_thread(&db, account, event).await;
    }

    let use_case = ThreadLinkUseCase::new(
        personal_secretary_mysql::build_mysql_thread_link_store(db.clone()),
        32,
        100_000,
        60,
    )
    .expect("valid link budgets");
    let run = use_case
        .run_once()
        .await
        .expect("scan structured references")
        .expect("events available");
    assert_eq!(run.events_read, 9);
    assert_eq!(run.candidates_created, 3);

    assert_eq!(candidate_count(&db, "exact_forward_source_key").await, 1);
    assert_eq!(candidate_count(&db, "exact_rich_content_key").await, 1);
    assert_eq!(candidate_count(&db, "explicit_file_version").await, 1);
    assert_eq!(candidate_count(&db, "exact_file_source_key").await, 0);
    assert_eq!(all_candidate_count(&db).await, 3);
    assert_eq!(account_candidate_count(&db, "account-b").await, 0);

    assert!(
        use_case
            .run_once()
            .await
            .expect("idempotent rescan")
            .is_none()
    );
    assert_eq!(all_candidate_count(&db).await, 3);

    let weak_insert = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_thread_link_hints \
             (hint_id, account_id, conversation_id, thread_id, source_event_id, \
              signal_kind, fingerprint_sha256) \
             SELECT ?, e.account_id, e.conversation_id, te.thread_id, e.source_event_id, \
                    'same_file_name', ? \
             FROM secretary_source_events e \
             JOIN secretary_thread_events te ON te.source_event_id = e.source_event_id \
             WHERE e.source_event_id = ?",
            vec![
                Uuid::new_v4().to_string().into(),
                "b".repeat(64).into(),
                weak_a.as_str().into(),
            ],
        ))
        .await;
    assert!(
        weak_insert.is_err(),
        "database must reject weak signal kinds"
    );

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM qqbot_test_schema_migrations WHERE migration_name = ?",
        [MIGRATION_NAME.into()],
    ))
    .await
    .expect("remove migration record for replay");
    common::try_replay_folded_migration(&db, MIGRATION_NAME)
        .await
        .expect("structured-reference migration must be replayable");
}

async fn insert_message(
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    account: &str,
    message_id: &str,
    conversation: &str,
    actor: &str,
    text: &str,
    segments: Vec<ContentSegment>,
) -> SourceEventId {
    inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::NapCat, account, message_id).unwrap(),
                ConversationRef::new(ConversationKind::Group, conversation).unwrap(),
                VerifiedActor::new(VerifiedActorKind::External, actor).unwrap(),
                1_800_000_000,
                text,
                segments,
            )
            .unwrap(),
        )
        .await
        .expect("insert message")
        .source_event_id()
        .clone()
}

async fn attach_to_distinct_thread(db: &DatabaseConnection, account: &str, event: &SourceEventId) {
    let thread_id = Uuid::new_v4().to_string();
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_event_threads \
             (thread_id, account_id, root_event_id, latest_event_id, \
              opened_at_unix_secs, latest_occurred_at_unix_secs) \
             SELECT ?, a.id, ?, ?, 1800000000, 1800000000 \
             FROM secretary_accounts a \
             WHERE a.source_channel = 'napcat' AND a.platform_account_id = ?",
            vec![
                thread_id.clone().into(),
                event.as_str().into(),
                event.as_str().into(),
                account.to_owned().into(),
            ],
        ))
        .await
        .expect("insert thread");
    assert_eq!(inserted.rows_affected(), 1);
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_events (source_event_id, thread_id) VALUES (?, ?)",
        [event.as_str().into(), thread_id.into()],
    ))
    .await
    .expect("attach event to thread");
}

async fn candidate_count(db: &DatabaseConnection, signal_kind: &str) -> u64 {
    common::scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_thread_link_candidates WHERE signal_kind = ?",
        vec![signal_kind.to_owned().into()],
    )
    .await
}

async fn all_candidate_count(db: &DatabaseConnection) -> u64 {
    common::scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_thread_link_candidates",
        Vec::new(),
    )
    .await
}

async fn account_candidate_count(db: &DatabaseConnection, account: &str) -> u64 {
    common::scalar_u64(
        db,
        "SELECT COUNT(*) AS value \
         FROM secretary_thread_link_candidates candidate \
         JOIN secretary_accounts account ON account.id = candidate.account_id \
         WHERE account.source_channel = 'napcat' AND account.platform_account_id = ?",
        vec![account.to_owned().into()],
    )
    .await
}
