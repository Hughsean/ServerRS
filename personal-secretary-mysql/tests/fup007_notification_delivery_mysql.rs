//! FUP-007 本地通知 Outbox 的账号隔离、租约 fencing、失败收敛和送达回执。

mod common;

use personal_secretary::{
    CommitmentMemory, CommitmentStatus, InboundEventStoreError, MemoryFact, MemoryFactId,
    MemoryFactStatus, MemoryPayload, NotificationFailureKind, NotificationId,
    NotificationLeaseToken, SourceAccountRef, ThreadActorRef,
};
use personal_secretary_mysql::build_mysql_follow_up_store;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn notification_delivery_state_machine_is_fenced_and_account_scoped() {
    let (db, schema) = common::isolated_db("_fup007").await;
    let scenario_db = db.clone();
    let result = tokio::spawn(async move { run_scenario(scenario_db).await }).await;
    common::drop_schema(&db, &schema).await;
    result.expect("FUP-007 MySQL scenario must complete");
}

async fn run_scenario(db: DatabaseConnection) {
    seed_account(&db, "fup007-a").await;
    seed_account(&db, "fup007-b").await;
    let account_a = common::account("fup007-a");
    let account_b = common::account("fup007-b");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_secs() as i64;

    let isolation_a = seed_notification(&db, "fup007-isolation-a", &account_a, now).await;
    let isolation_b = seed_notification(&db, "fup007-isolation-b", &account_b, now).await;
    let store = build_mysql_follow_up_store(db.clone());

    let claim_a = store
        .claim_due_notification(&account_a, now, 60)
        .await
        .expect("account A claim")
        .expect("account A must have a due notification");
    assert_eq!(claim_a.notification_id.as_str(), isolation_a.as_str());
    assert!(
        store
            .claim_due_notification(&account_a, now, 60)
            .await
            .expect("account A second claim")
            .is_none()
    );
    assert_eq!(status(&db, isolation_b.as_str()).await, "pending");
    store
        .mark_notification_delivered(
            &claim_a.notification_id,
            &claim_a.lease_token,
            "platform-a-1",
        )
        .await
        .expect("valid delivery receipt");
    assert_eq!(status(&db, isolation_a.as_str()).await, "delivered");
    assert_eq!(
        platform_message_id(&db, isolation_a.as_str()).await,
        "platform-a-1"
    );
    assert!(matches!(
        store
            .mark_notification_delivered(
                &claim_a.notification_id,
                &claim_a.lease_token,
                "platform-a-duplicate",
            )
            .await,
        Err(InboundEventStoreError::LeaseLost)
    ));

    let forged_id = seed_notification(&db, "fup007-forged", &account_a, now).await;
    let forged_claim = store
        .claim_due_notification(&account_a, now, 60)
        .await
        .expect("forged claim")
        .expect("forged fixture must be claimable");
    assert_eq!(forged_claim.notification_id.as_str(), forged_id.as_str());
    let forged = NotificationLeaseToken::generate();
    assert!(matches!(
        store
            .mark_notification_failed(
                &forged_claim.notification_id,
                &forged,
                "transport_rejected",
                NotificationFailureKind::Permanent,
            )
            .await,
        Err(InboundEventStoreError::LeaseLost)
    ));
    assert_eq!(status(&db, forged_id.as_str()).await, "claimed");
    store
        .mark_notification_failed(
            &forged_claim.notification_id,
            &forged_claim.lease_token,
            "transport_rejected",
            NotificationFailureKind::Permanent,
        )
        .await
        .expect("valid permanent failure");
    assert_eq!(status(&db, forged_id.as_str()).await, "failed");

    let retry_id = seed_notification(&db, "fup007-retry", &account_a, now).await;
    let retry_claim = store
        .claim_due_notification(&account_a, now, 60)
        .await
        .expect("retry claim")
        .expect("retry fixture must be claimable");
    store
        .mark_notification_failed(
            &retry_claim.notification_id,
            &retry_claim.lease_token,
            "temporary_unavailable",
            NotificationFailureKind::Retryable,
        )
        .await
        .expect("retryable failure");
    assert_eq!(status(&db, retry_id.as_str()).await, "pending");
    assert_eq!(
        last_error_code(&db, retry_id.as_str()).await,
        "temporary_unavailable"
    );
    assert!(scheduled_at(&db, retry_id.as_str()).await > now);

    let unknown_id = seed_notification(&db, "fup007-unknown", &account_a, now).await;
    let unknown_claim = store
        .claim_due_notification(&account_a, now, 60)
        .await
        .expect("unknown commit claim")
        .expect("unknown fixture must be claimable");
    store
        .mark_notification_failed(
            &unknown_claim.notification_id,
            &unknown_claim.lease_token,
            "response_ambiguous",
            NotificationFailureKind::UnknownCommit,
        )
        .await
        .expect("unknown commit failure");
    assert_eq!(status(&db, unknown_id.as_str()).await, "unknown_commit");
    assert!(lease_token(&db, unknown_id.as_str()).await.is_none());

    let expired_id = seed_notification(&db, "fup007-expired", &account_a, now).await;
    let expired_claim = store
        .claim_due_notification(&account_a, now, 1)
        .await
        .expect("expired claim")
        .expect("expired fixture must be claimable");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_notification_outbox SET lease_expires_at = UTC_TIMESTAMP(6) - INTERVAL 60 SECOND WHERE notification_id = ?",
        [expired_id.as_str().into()],
    ))
    .await
    .expect("expire lease");
    assert!(matches!(
        store
            .mark_notification_delivered(
                &expired_claim.notification_id,
                &expired_claim.lease_token,
                "too-late",
            )
            .await,
        Err(InboundEventStoreError::LeaseLost)
    ));
    assert_eq!(status(&db, expired_id.as_str()).await, "claimed");
    assert!(
        store
            .claim_due_notification(&account_a, now, 60)
            .await
            .expect("expired claim reconciliation")
            .is_none()
    );
    assert_eq!(status(&db, expired_id.as_str()).await, "unknown_commit");
}

async fn seed_account(db: &DatabaseConnection, platform_account_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_accounts (source_channel, platform_account_id, status) VALUES ('napcat', ?, 'active')",
        [platform_account_id.into()],
    ))
    .await
    .expect("seed account");
}

async fn seed_notification(
    db: &DatabaseConnection,
    id: &str,
    account: &SourceAccountRef,
    now: i64,
) -> NotificationId {
    let fact_id = format!("{id}-fact");
    let follow_up_id = format!("{id}-follow-up");
    let fact_json = serde_json::to_string(&MemoryFact {
        fact_id: MemoryFactId::new(&fact_id).unwrap(),
        account: account.clone(),
        subject_key: format!("{id}-subject"),
        payload: MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: account.clone(),
                actor_id: "promisor".into(),
                platform_identity_kind: None,
            },
            beneficiary: ThreadActorRef {
                account: account.clone(),
                actor_id: "beneficiary".into(),
                platform_identity_kind: None,
            },
            action: "complete task".into(),
            due_at_unix_secs: Some(now),
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 10_000,
        source_event_ids: Vec::new(),
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    })
    .unwrap();
    let account_id = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT id FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
            [account.account_id.clone().into()],
        ))
        .await
        .expect("account lookup")
        .expect("account exists")
        .try_get::<u64>("", "id")
        .expect("account id");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_memory_facts (fact_id, account_id, fact_kind, subject_key, fact_json, fact_status, confidence_bps) VALUES (?, ?, 'commitment', ?, ?, 'confirmed', 10000)",
        vec![
            fact_id.clone().into(),
            account_id.into(),
            format!("{id}-subject").into(),
            fact_json.into(),
        ],
    ))
    .await
    .expect("seed memory fact");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_follow_up_items (follow_up_id, account_id, source_memory_fact_id, source_version, reason_code, due_at_unix_secs, status) VALUES (?, ?, ?, 1, 'commitment_due', ?, 'scheduled')",
        vec![
            follow_up_id.clone().into(),
            account_id.into(),
            fact_id.into(),
            now.into(),
        ],
    ))
    .await
    .expect("seed follow-up");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_outbox (notification_id, account_id, follow_up_id, scheduled_at_unix_secs, notification_kind, payload_json, delivery_status) VALUES (?, ?, ?, ?, 'owner_reminder', JSON_OBJECT(), 'pending')",
        vec![id.into(), account_id.into(), follow_up_id.into(), now.into()],
    ))
    .await
    .expect("seed notification");
    NotificationId::new(id).unwrap()
}

async fn status(db: &DatabaseConnection, id: &str) -> String {
    scalar_string(db, "SELECT delivery_status AS value FROM secretary_notification_outbox WHERE notification_id = ?", id).await
}

async fn last_error_code(db: &DatabaseConnection, id: &str) -> String {
    scalar_string(db, "SELECT last_error_code AS value FROM secretary_notification_outbox WHERE notification_id = ?", id).await
}

async fn platform_message_id(db: &DatabaseConnection, id: &str) -> String {
    scalar_string(db, "SELECT platform_message_id AS value FROM secretary_notification_outbox WHERE notification_id = ?", id).await
}

async fn scheduled_at(db: &DatabaseConnection, id: &str) -> i64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT scheduled_at_unix_secs AS value FROM secretary_notification_outbox WHERE notification_id = ?",
            [id.into()],
        ))
        .await
        .expect("scheduled lookup")
        .expect("scheduled row");
    row.try_get::<i64>("", "value").expect("scheduled value")
}

async fn lease_token(db: &DatabaseConnection, id: &str) -> Option<String> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT lease_token AS value FROM secretary_notification_outbox WHERE notification_id = ?",
            [id.into()],
        ))
        .await
        .expect("lease lookup")
        .expect("lease row");
    row.try_get::<Option<String>>("", "value")
        .expect("lease value")
}

async fn scalar_string(db: &DatabaseConnection, sql: &str, id: &str) -> String {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            [id.into()],
        ))
        .await
        .expect("scalar lookup")
        .expect("scalar row");
    row.try_get::<String>("", "value").expect("scalar value")
}
