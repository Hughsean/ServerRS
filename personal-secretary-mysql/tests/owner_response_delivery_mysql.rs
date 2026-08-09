mod common;

use std::sync::Arc;

use personal_secretary::{
    ActionLeaseToken, ActionRunId, ConversationKind, ConversationRef, InboundEventStoreError,
    InboundMessageEnvelope, MessageSource, NotificationFailureKind, OwnerBinding,
    OwnerResponseDeliveryScope, OwnerResponseDraft, OwnerResponseLeaseToken, OwnerResponseTarget,
    ResponseSegment, SourceAccountRef, SourceMessageRef, VerifiedActor, VerifiedActorKind,
};
use personal_secretary_mysql::{
    build_mysql_action_store, build_mysql_inbound_event_store, build_mysql_owner_binding_store,
    build_mysql_owner_response_delivery_store,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

const MANAGED_ACCOUNT: &str = "owner-response-managed";
const COMMAND_ACCOUNT: &str = "owner-response-bot";
const OWNER_ID: &str = "owner-response-openid";

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn owner_response_delivery_is_authorized_fenced_and_fail_closed() {
    let (db, schema) = common::isolated_db("_owner_response").await;
    let scenario_db = db.clone();
    let result = tokio::spawn(async move { run_scenario(scenario_db).await }).await;
    common::drop_schema(&db, &schema).await;
    result.expect("Owner response MySQL scenario must complete");
}

async fn run_scenario(db: DatabaseConnection) {
    seed_managed_account(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let now = unix_now();
    let first_response = seed_completed_response(&db, &inbound, "command-1", now).await;
    seed_and_rotate_active_binding(&db).await;

    let store = build_mysql_owner_response_delivery_store(db.clone());
    let scope = OwnerResponseDeliveryScope::new(
        SourceAccountRef::new(MessageSource::NapCat, MANAGED_ACCOUNT).unwrap(),
        SourceAccountRef::new(MessageSource::QqOpenPlatform, COMMAND_ACCOUNT).unwrap(),
        OWNER_ID,
    )
    .unwrap();
    let claim = store
        .claim_pending_response(&scope, now, 60, 240)
        .await
        .expect("claim succeeds")
        .expect("authorized Owner response is claimable");
    assert_eq!(claim.response_id.as_str(), first_response);
    assert_eq!(claim.reply_to_platform_message_id, "command-1");
    assert_eq!(claim.target, OwnerResponseTarget::C2c);

    let forged = OwnerResponseLeaseToken::generate();
    assert!(matches!(
        store
            .mark_response_delivered(&claim.response_id, &forged, "provider-response-forged")
            .await,
        Err(InboundEventStoreError::LeaseLost)
    ));
    store
        .mark_response_delivered(
            &claim.response_id,
            &claim.lease_token,
            "provider-response-1",
        )
        .await
        .expect("valid delivery receipt");
    assert_eq!(status(&db, &first_response).await, "delivered");

    let unknown_response = seed_completed_response(&db, &inbound, "command-2", now + 1).await;
    let unknown_claim = store
        .claim_pending_response(&scope, now + 1, 60, 240)
        .await
        .unwrap()
        .expect("second response claim");
    assert_eq!(unknown_claim.response_id.as_str(), unknown_response);
    store
        .mark_response_failed(
            &unknown_claim.response_id,
            &unknown_claim.lease_token,
            "transport_unknown",
            NotificationFailureKind::UnknownCommit,
        )
        .await
        .expect("unknown commit is terminal");
    assert_eq!(status(&db, &unknown_response).await, "unknown_commit");
    assert!(
        store
            .claim_pending_response(&scope, now + 2, 60, 240)
            .await
            .unwrap()
            .is_none(),
        "unknown commits must never be retried"
    );

    // 已处理的最早 100 行不得让后续新 Response 饥饿。物化必须先排除已有 Outbox，再 LIMIT。
    for index in 0..100 {
        let response_id =
            seed_completed_response(&db, &inbound, &format!("history-{index}"), now + 10 + index)
                .await;
        mark_as_historical_delivered(&db, &response_id).await;
    }
    let after_history = seed_completed_response(&db, &inbound, "after-history", now + 110).await;
    let after_history_claim = store
        .claim_pending_response(&scope, now + 110, 60, 240)
        .await
        .unwrap()
        .expect("new response after 100 delivered rows must remain claimable");
    assert_eq!(after_history_claim.response_id.as_str(), after_history);
    store
        .mark_response_delivered(
            &after_history_claim.response_id,
            &after_history_claim.lease_token,
            "provider-response-after-history",
        )
        .await
        .unwrap();

    let group_response =
        seed_completed_group_response(&db, &inbound, "group-command", now + 111).await;
    let group_claim = store
        .claim_pending_response(&scope, now + 111, 60, 240)
        .await
        .unwrap()
        .expect("Owner @Bot group response is claimable");
    assert_eq!(group_claim.response_id.as_str(), group_response);
    assert_eq!(
        group_claim.target,
        OwnerResponseTarget::Group {
            group_openid: "group-target".into()
        }
    );
    store
        .mark_response_delivered(
            &group_claim.response_id,
            &group_claim.lease_token,
            "provider-group-response",
        )
        .await
        .unwrap();

    let revoked_response = seed_completed_response(&db, &inbound, "command-3", now + 112).await;
    db.execute_raw(Statement::from_string(
        DatabaseBackend::MySql,
        "UPDATE secretary_owner_bindings SET status = 'revoked'".to_owned(),
    ))
    .await
    .expect("revoke binding");
    assert!(
        store
            .claim_pending_response(&scope, now + 2, 60, 240)
            .await
            .unwrap()
            .is_none(),
        "revoked Owner binding must fail closed"
    );
    assert_eq!(outbox_count(&db, &revoked_response).await, 0);
}

async fn mark_as_historical_delivered(db: &DatabaseConnection, response_id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_action_responses SET created_at = UTC_TIMESTAMP(6) - INTERVAL 1 DAY WHERE response_id = ?",
        [response_id.into()],
    ))
    .await
    .expect("backdate historical response");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_owner_response_outbox (response_id, delivery_status, attempts, delivered_at) VALUES (?, 'delivered', 1, UTC_TIMESTAMP(6))",
        [response_id.into()],
    ))
    .await
    .expect("seed delivered historical outbox row");
}

async fn seed_managed_account(db: &DatabaseConnection) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_accounts (source_channel, platform_account_id, status) VALUES ('napcat', ?, 'active')",
        [MANAGED_ACCOUNT.into()],
    ))
    .await
    .expect("seed managed account");
}

async fn seed_and_rotate_active_binding(db: &DatabaseConnection) {
    let store = build_mysql_owner_binding_store(db.clone());
    for owner in ["superseded-owner", OWNER_ID] {
        store
            .ensure_owner_binding(&OwnerBinding {
                managed_account: SourceAccountRef::new(MessageSource::NapCat, MANAGED_ACCOUNT)
                    .unwrap(),
                command_account: SourceAccountRef::new(
                    MessageSource::QqOpenPlatform,
                    COMMAND_ACCOUNT,
                )
                .unwrap(),
                owner_actor_id: owner.into(),
            })
            .await
            .expect("rotate Owner binding");
    }
    assert_eq!(
        common::scalar_u64(
            db,
            "SELECT COUNT(*) AS value FROM secretary_owner_bindings WHERE status = 'active'",
            Vec::new(),
        )
        .await,
        1,
        "Owner rotation must leave exactly one active binding"
    );
}

async fn seed_completed_response(
    db: &DatabaseConnection,
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    platform_message_id: &str,
    occurred_at: i64,
) -> String {
    seed_completed_response_for_target(
        db,
        inbound,
        platform_message_id,
        occurred_at,
        "c2c_message",
        None,
    )
    .await
}

async fn seed_completed_group_response(
    db: &DatabaseConnection,
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    platform_message_id: &str,
    occurred_at: i64,
) -> String {
    seed_completed_response_for_target(
        db,
        inbound,
        platform_message_id,
        occurred_at,
        "group_at_message",
        Some("group-target"),
    )
    .await
}

async fn seed_completed_response_for_target(
    db: &DatabaseConnection,
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    platform_message_id: &str,
    occurred_at: i64,
    event_kind: &str,
    group_openid: Option<&str>,
) -> String {
    let command = InboundMessageEnvelope::new(
        SourceMessageRef::new(
            MessageSource::QqOpenPlatform,
            COMMAND_ACCOUNT,
            platform_message_id,
        )
        .unwrap(),
        ConversationRef::new(
            ConversationKind::OwnerControl,
            group_openid.unwrap_or(OWNER_ID),
        )
        .unwrap(),
        VerifiedActor::new(VerifiedActorKind::Owner, OWNER_ID).unwrap(),
        occurred_at,
        "owner command",
        Vec::new(),
    )
    .unwrap();
    let source_event_id = inbound
        .insert_message_if_absent(&command)
        .await
        .expect("insert OwnerCommand")
        .source_event_id()
        .clone();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"INSERT INTO secretary_qq_raw_events
           (source_event_id, app_id, event_kind, envelope_json)
           VALUES (?, ?, ?, CAST(? AS JSON))"#,
        [
            source_event_id.as_str().into(),
            COMMAND_ACCOUNT.into(),
            event_kind.into(),
            serde_json::json!({"d": {"group_openid": group_openid}})
                .to_string()
                .into(),
        ],
    ))
    .await
    .expect("insert authoritative raw Gateway event");

    let run_id = ActionRunId::for_owner_command(&source_event_id, platform_message_id);
    let lease = ActionLeaseToken::generate();
    common::insert_action_run(
        db,
        &SourceAccountRef::new(MessageSource::NapCat, MANAGED_ACCOUNT).unwrap(),
        &run_id,
        &source_event_id,
        &lease,
    )
    .await;
    let draft = OwnerResponseDraft::new(
        vec![ResponseSegment::Summary {
            text: "owner response".into(),
        }],
        vec![source_event_id],
        occurred_at,
    )
    .unwrap();
    build_mysql_action_store(db.clone())
        .mark_completed(&run_id, &lease, Some(&draft))
        .await
        .expect("complete Action Run with response draft");
    common::scalar_string(
        db,
        "SELECT response_id AS value FROM secretary_action_responses WHERE run_id = ?",
        vec![run_id.as_str().into()],
    )
    .await
}

async fn status(db: &DatabaseConnection, response_id: &str) -> String {
    common::scalar_string(
        db,
        "SELECT delivery_status AS value FROM secretary_owner_response_outbox WHERE response_id = ?",
        vec![response_id.into()],
    )
    .await
}

async fn outbox_count(db: &DatabaseConnection, response_id: &str) -> u64 {
    common::scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_owner_response_outbox WHERE response_id = ?",
        vec![response_id.into()],
    )
    .await
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
