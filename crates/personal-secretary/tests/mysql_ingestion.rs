use personal_secretary::{
    ConnectionEndReason, ContentSegment, ConversationKind, ConversationRef, InboundMessageEnvelope,
    IngestMessageOutcome, IngestionGapReason, MessageSource, SourceAccountRef, SourceMessageRef,
    VerifiedActor, VerifiedActorKind, build_mysql_inbound_event_store,
};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};
use uuid::Uuid;

fn message(
    account_id: &str,
    message_id: &str,
    segments: Vec<ContentSegment>,
) -> InboundMessageEnvelope {
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, account_id, message_id).unwrap(),
        ConversationRef::new(ConversationKind::Group, "group-1").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "sender-1").unwrap(),
        1_800_000_000,
        "@user 请确认",
        segments,
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn mysql_store_is_idempotent_and_resolves_reply_mentions() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    let store = build_mysql_inbound_event_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let account_a = format!("account-a-{run_id}");
    let account_b = format!("account-b-{run_id}");

    let parent = message(&account_a, "message-1", Vec::new());
    let accepted_parent = store.insert_message_if_absent(&parent).await.unwrap();
    let parent_id = match accepted_parent {
        IngestMessageOutcome::Accepted {
            source_event_id, ..
        } => source_event_id,
        outcome => panic!("expected accepted parent, got {outcome:?}"),
    };

    let duplicate = store.insert_message_if_absent(&parent).await.unwrap();
    assert_eq!(duplicate.source_event_id(), &parent_id);
    assert!(matches!(duplicate, IngestMessageOutcome::Duplicate { .. }));

    let reply = message(
        &account_a,
        "message-2",
        vec![
            ContentSegment::Mention {
                actor_id: "member-2".into(),
            },
            ContentSegment::MentionAll,
            ContentSegment::Reply {
                platform_message_id: "message-1".into(),
            },
        ],
    );
    let accepted_reply = store.insert_message_if_absent(&reply).await.unwrap();
    let reply_id = match accepted_reply {
        IngestMessageOutcome::Accepted {
            source_event_id,
            reply_to_event_id,
        } => {
            assert_eq!(reply_to_event_id.as_ref(), Some(&parent_id));
            source_event_id
        }
        outcome => panic!("expected accepted reply, got {outcome:?}"),
    };

    let other_account = message(&account_b, "message-1", Vec::new());
    assert!(matches!(
        store
            .insert_message_if_absent(&other_account)
            .await
            .unwrap(),
        IngestMessageOutcome::Accepted { .. }
    ));

    let event_count = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT COUNT(*) AS count \
             FROM secretary_source_events event \
             INNER JOIN secretary_accounts account ON account.id = event.account_id \
             WHERE account.platform_account_id IN (?, ?)",
            [account_a.into(), account_b.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "count")
        .unwrap();
    assert_eq!(event_count, 3);

    let content = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT mentioned_actor_ids, mention_all FROM secretary_message_contents WHERE source_event_id = ?",
            [reply_id.as_str().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let mentioned = content
        .try_get::<serde_json::Value>("", "mentioned_actor_ids")
        .unwrap();
    let mention_all = content.try_get::<bool>("", "mention_all").unwrap();
    assert_eq!(mentioned, serde_json::json!(["member-2"]));
    assert!(mention_all);
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn mysql_store_tracks_connection_cursor_and_uncertain_gap() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    let store = build_mysql_inbound_event_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("continuity-{run_id}");
    let account = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();

    let first_epoch = store.begin_connection(&account).await.unwrap();
    store.mark_connection_connected(&first_epoch).await.unwrap();
    let overflow_gap = store
        .mark_connection_uncertain(&first_epoch, IngestionGapReason::QueueOverflow)
        .await
        .unwrap();
    let repeated_overflow_gap = store
        .mark_connection_uncertain(&first_epoch, IngestionGapReason::QueueOverflow)
        .await
        .unwrap();
    assert_eq!(overflow_gap, repeated_overflow_gap);

    let observed = message(&account_id, "message-1", Vec::new()).observed_in(first_epoch.clone());
    let accepted = store.insert_message_if_absent(&observed).await.unwrap();
    let source_event_id = accepted.source_event_id().as_str().to_owned();

    let cursor_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value \
         FROM secretary_ingestion_cursors c \
         INNER JOIN secretary_accounts account ON account.id = c.account_id \
         WHERE account.platform_account_id = ?",
        [&account_id],
    )
    .await;
    assert_eq!(
        cursor_count, 2,
        "account and conversation cursors must exist"
    );

    let linked_event_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_event_ingestion \
         WHERE source_event_id = ? AND connection_epoch_id = ?",
        [source_event_id.as_str(), first_epoch.as_str()],
    )
    .await;
    assert_eq!(linked_event_count, 1);

    let first_gap = store
        .finish_connection(&first_epoch, ConnectionEndReason::TransportError)
        .await
        .unwrap()
        .expect("a connected epoch must create an uncertain gap");
    assert_eq!(first_gap, overflow_gap);
    let repeated_finish = store
        .finish_connection(&first_epoch, ConnectionEndReason::TransportError)
        .await
        .unwrap()
        .expect("finishing an epoch twice must return its existing gap");
    assert_eq!(first_gap, repeated_finish);

    let open_gap_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_ingestion_gaps \
         WHERE gap_id = ? AND status = 'uncertain' AND reason = 'queue_overflow' \
         AND gap_ended_at IS NULL",
        [first_gap.as_str()],
    )
    .await;
    assert_eq!(open_gap_count, 1);

    let second_epoch = store.begin_connection(&account).await.unwrap();
    store
        .mark_connection_connected(&second_epoch)
        .await
        .unwrap();
    let closed_but_unverified_gap_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_ingestion_gaps \
         WHERE gap_id = ? AND status = 'uncertain' AND gap_ended_at IS NOT NULL",
        [first_gap.as_str()],
    )
    .await;
    assert_eq!(closed_but_unverified_gap_count, 1);

    assert!(
        store
            .finish_connection(&second_epoch, ConnectionEndReason::ProcessShutdown)
            .await
            .unwrap()
            .is_some(),
        "shutdown also creates an uncertain window until the next verified backfill"
    );
}

async fn scalar_i64<const N: usize>(
    db: &sea_orm::DatabaseConnection,
    sql: &str,
    values: [&str; N],
) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        sql,
        values.map(Into::into),
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<i64>("", "value")
    .unwrap()
}
