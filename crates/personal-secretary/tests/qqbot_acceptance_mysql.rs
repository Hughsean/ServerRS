//! 独立 QQBot 验收测试。
//!
//! 这些测试描述跨模块业务不变量，不复用生产实现中的 helper，也不把 Fake Store 当作
//! 生产闭环证据。除纯身份约束外，测试必须指向隔离 MySQL schema，并由
//! `scripts/verify-qqbot-acceptance.ps1` 逐项运行。

use personal_secretary::{
    ArtifactEnvelope, ArtifactId, ArtifactKind, ArtifactUseCase, ConnectionEndReason,
    ContentSegment, ContentTrustLevel, ConversationKind, ConversationRef, ConversationScope,
    DirectoryEvidence, DirectorySnapshot, DirectorySnapshotId, DirectorySourceApi, DirectoryStatus,
    InboundMessageEnvelope, MessageSource, RecallCorrelationKey, RecallEvent, RecallEventId,
    RecallKind, RecallUseCase, ScopeKind, SourceAccountRef, SourceMessageRef, TombstoneStatus,
    VerifiedActor, VerifiedActorKind, build_mysql_artifact_store, build_mysql_directory_store,
    build_mysql_inbound_event_store, build_mysql_recall_store,
};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

fn account(subject: &str) -> SourceAccountRef {
    SourceAccountRef::new(MessageSource::NapCat, subject).expect("valid account fixture")
}

fn group(group_id: &str) -> ConversationRef {
    ConversationRef::new(ConversationKind::Group, group_id).expect("valid group fixture")
}

fn message(account_subject: &str, group_id: &str, message_id: &str) -> InboundMessageEnvelope {
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, account_subject, message_id)
            .expect("valid source fixture"),
        group(group_id),
        VerifiedActor::new(VerifiedActorKind::External, "acceptance-sender")
            .expect("valid actor fixture"),
        1_800_000_000,
        "acceptance message",
        Vec::new(),
    )
    .expect("valid inbound fixture")
}

fn recall(
    recall_event_id: impl Into<String>,
    account_subject: &str,
    group_id: &str,
    message_id: &str,
) -> RecallEvent {
    let account = account(account_subject);
    RecallEvent {
        recall_event_id: RecallEventId::new(recall_event_id).expect("valid recall id fixture"),
        account: account.clone(),
        kind: RecallKind::Group,
        correlation: RecallCorrelationKey::new(
            account,
            MessageSource::NapCat,
            group(group_id),
            message_id,
        )
        .expect("valid recall correlation fixture"),
        operator_platform_id: Some("acceptance-operator".into()),
        occurred_at_unix_secs: 1_800_000_100,
    }
}

async fn isolated_db() -> DatabaseConnection {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must point to an isolated qqbot_accept_* schema");
    let schema = url
        .split('?')
        .next()
        .and_then(|value| value.rsplit('/').next())
        .unwrap_or_default();
    assert!(
        schema.starts_with("qqbot_accept_"),
        "refusing to run acceptance tests against non-isolated schema: {schema}"
    );
    let db = Database::connect(url)
        .await
        .expect("connect isolated acceptance MySQL");
    apply_qqbot_migrations(&db).await;
    db
}

async fn apply_qqbot_migrations(db: &DatabaseConnection) {
    let migrations_dir = format!(
        "{}/../../apps/qqbot-server/database/migrations",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(&migrations_dir)
        .unwrap_or_else(|error| panic!("failed to read migrations directory: {error}"))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect();
    entries.sort_by_key(|path| migration_order(path));
    for path in entries {
        let migration_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if migration_name.contains("_owner_agenda.sql")
            && db
                .query_one_raw(Statement::from_string(
                    DatabaseBackend::MySql,
                    "SELECT 1 FROM information_schema.statistics WHERE table_schema = DATABASE() AND table_name = 'secretary_notification_outbox' AND index_name = 'uk_secretary_notification_agenda' LIMIT 1",
                ))
                .await
                .expect("agenda migration sentinel query failed")
                .is_some()
        {
            continue;
        }
        let sql = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let stripped = sql
            .lines()
            .map(|line| line.split_once("--").map_or(line, |(prefix, _)| prefix))
            .collect::<Vec<_>>()
            .join("\n");
        for statement in stripped
            .split(';')
            .map(str::trim)
            .filter(|sql| !sql.is_empty())
        {
            db.execute_raw(Statement::from_string(DatabaseBackend::MySql, statement))
                .await
                .unwrap_or_else(|error| panic!("migration {} failed: {error}", path.display()));
        }
    }
}

fn migration_order(path: &std::path::Path) -> u8 {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    match name {
        name if name.contains("_ingestion.sql") => 0,
        name if name.contains("_continuity.sql") => 1,
        name if name.contains("_backfill.sql") => 2,
        name if name.contains("_threads.sql") => 3,
        name if name.contains("_thread_links.sql") => 4,
        name if name.contains("_thread_semantics.sql") => 5,
        name if name.contains("_thread_mutations.sql") => 6,
        name if name.contains("_thread_revisions.sql") => 7,
        name if name.contains("_memory.sql") => 8,
        name if name.contains("_memory_controls_followups.sql") => 9,
        name if name.contains("_qq_open_platform.sql") => 10,
        name if name.contains("_action_planner.sql") => 11,
        name if name.contains("_action_planner_hardening.sql") => 12,
        name if name.contains("_directory.sql") => 13,
        name if name.contains("_gap_freeze_hardening.sql") => 14,
        name if name.contains("_event_type_recall.sql") => 15,
        name if name.contains("_recall.sql") => 16,
        name if name.contains("_artifacts.sql") => 17,
        name if name.contains("_recall_inbox.sql") => 18,
        name if name.contains("_artifact_derivations.sql") => 19,
        name if name.contains("_owner_agenda.sql") => 20,
        _ => 99,
    }
}

async fn scalar_u64(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> u64 {
    let value = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .await
        .expect("acceptance query must execute")
        .expect("acceptance query must return one row")
        .try_get::<i64>("", "value")
        .expect("MySQL COUNT must decode as signed BIGINT");
    u64::try_from(value).expect("acceptance count must not be negative")
}

async fn scalar_string(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> String {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        sql,
        values,
    ))
    .await
    .expect("acceptance query must execute")
    .expect("acceptance query must return one row")
    .try_get::<String>("", "value")
    .expect("acceptance scalar must decode as string")
}

#[test]
#[ignore = "executed only by verify-qqbot-acceptance.ps1"]
fn acceptance_recall_identity_rejects_database_truncation() {
    assert!(RecallEventId::new("recall-group-1839717811-671260344-1234567890123456789").is_err());
    assert!(RecallEventId::new(Uuid::new_v4().to_string()).is_ok());
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_recall_is_persisted_as_source_event() {
    let db = isolated_db().await;
    let account_subject = format!("accept-recall-source-{}", Uuid::new_v4().simple());
    let inbound = build_mysql_inbound_event_store(db.clone());
    inbound
        .begin_connection(&account(&account_subject))
        .await
        .expect("account bootstrap must succeed");
    let recall_id = Uuid::new_v4().to_string();
    RecallUseCase::new(build_mysql_recall_store(db.clone()))
        .handle_recall(&recall(
            &recall_id,
            &account_subject,
            "671260344",
            "987654321012345678",
        ))
        .await
        .expect("recall persistence must succeed");
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_source_events WHERE source_event_id = ?",
            vec![recall_id.into()]
        )
        .await,
        1
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_pending_recall_auto_applies_after_message_ingestion() {
    let db = isolated_db().await;
    let account_subject = format!("accept-pending-{}", Uuid::new_v4().simple());
    let group_id = "671260344";
    let message_id = "887766554433221100";
    let inbound = build_mysql_inbound_event_store(db.clone());
    inbound
        .begin_connection(&account(&account_subject))
        .await
        .expect("account bootstrap must succeed");
    let status = RecallUseCase::new(build_mysql_recall_store(db.clone()))
        .handle_recall(&recall(
            Uuid::new_v4().to_string(),
            &account_subject,
            group_id,
            message_id,
        ))
        .await
        .expect("pending recall persistence must succeed");
    assert_eq!(status, TombstoneStatus::Pending);
    inbound
        .insert_message_if_absent(&message(&account_subject, group_id, message_id))
        .await
        .expect("original message ingestion must succeed");
    assert_eq!(
        scalar_string(
            &db,
            "SELECT status AS value FROM secretary_message_tombstones WHERE correlation_key = ?",
            vec![format!("napcat:{account_subject}:group:{group_id}:{message_id}").into()]
        )
        .await,
        "applied"
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_recall_propagates_to_existing_artifacts() {
    let db = isolated_db().await;
    let account_subject = format!("accept-artifact-{}", Uuid::new_v4().simple());
    let group_id = "671260344";
    let message_id = "112233445566778899";
    let source_event_id = build_mysql_inbound_event_store(db.clone())
        .insert_message_if_absent(&message(&account_subject, group_id, message_id))
        .await
        .expect("source message ingestion must succeed")
        .source_event_id()
        .clone();
    let artifact_id = ArtifactId::new(Uuid::new_v4().to_string()).expect("valid artifact id");
    let envelope = ArtifactEnvelope::new(
        artifact_id.clone(),
        account(&account_subject),
        source_event_id,
        group(group_id),
        ArtifactKind::Image,
        "platform-file-reference",
        ContentTrustLevel::Normal,
        1_800_000_000,
        Some(1_800_003_600),
    )
    .expect("valid artifact envelope");
    ArtifactUseCase::new(build_mysql_artifact_store(db.clone()))
        .create(&envelope)
        .await
        .expect("artifact creation must succeed");
    RecallUseCase::new(build_mysql_recall_store(db.clone()))
        .handle_recall(&recall(
            Uuid::new_v4().to_string(),
            &account_subject,
            group_id,
            message_id,
        ))
        .await
        .expect("recall persistence must succeed");
    assert_eq!(
        scalar_string(
            &db,
            "SELECT availability AS value FROM secretary_artifacts WHERE artifact_id = ?",
            vec![artifact_id.as_str().into()]
        )
        .await,
        "recalled"
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_gap_creation_freezes_latest_directory_snapshot() {
    let db = isolated_db().await;
    let account_subject = format!("accept-directory-{}", Uuid::new_v4().simple());
    let account = account(&account_subject);
    let inbound = build_mysql_inbound_event_store(db.clone());
    let directory = build_mysql_directory_store(db.clone());
    let epoch = inbound
        .begin_connection(&account)
        .await
        .expect("connection epoch must start");
    directory
        .snapshot_directory(&DirectorySnapshot {
            snapshot_id: DirectorySnapshotId::new(Uuid::new_v4().to_string())
                .expect("valid snapshot id"),
            account: account.clone(),
            source_api: DirectorySourceApi::FriendGroupRecent,
            status: DirectoryStatus::KnownScopesComplete,
            evidence: DirectoryEvidence {
                source_api: Some(DirectorySourceApi::FriendGroupRecent),
                group_count: 1,
                probed_at_unix_secs: 1_800_000_000,
                ..DirectoryEvidence::default()
            },
            scopes: vec![ConversationScope {
                conversation: group("671260344"),
                scope_kind: ScopeKind::Group,
                boundary: None,
                display_name: Some("acceptance-group".into()),
            }],
            created_at_unix_secs: 1_800_000_000,
        })
        .await
        .expect("directory snapshot must persist");
    inbound
        .mark_connection_connected(&epoch)
        .await
        .expect("connection must become connected");
    let gap_id = inbound
        .finish_connection(&epoch, ConnectionEndReason::TransportError)
        .await
        .expect("connection finish must succeed")
        .expect("connected epoch must produce a gap");
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_directory_gap_freeze WHERE gap_id = ?",
            vec![gap_id.as_str().into()]
        )
        .await,
        1
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_artifact_poison_job_fails_without_starving_later_work() {
    let db = isolated_db().await;
    let account_subject = format!("accept-artifact-poison-{}", Uuid::new_v4().simple());
    db.execute_raw(Statement::from_string(
        DatabaseBackend::MySql,
        "UPDATE secretary_artifact_derivations SET status = 'completed' WHERE status = 'pending'",
    ))
    .await
    .expect("test must isolate its derivation queue");
    let inbound = build_mysql_inbound_event_store(db.clone());
    let poison = inbound
        .insert_message_if_absent(&message(&account_subject, "671260344", "poison"))
        .await
        .expect("poison source message must persist")
        .source_event_id()
        .clone();
    let normal = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &account_subject, "normal")
            .expect("normal source"),
        group("671260344"),
        VerifiedActor::new(VerifiedActorKind::External, "acceptance-sender").expect("actor"),
        1_800_000_001,
        "image",
        vec![ContentSegment::Media {
            kind: personal_secretary::MediaKind::Image,
            source_key: "normal-image-key".into(),
            source_url: None,
            display_name: None,
        }],
    )
    .expect("normal inbound envelope");
    let normal = inbound
        .insert_message_if_absent(&normal)
        .await
        .expect("normal source message must persist")
        .source_event_id()
        .clone();
    db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::MySql, "UPDATE secretary_message_contents SET segments = CAST(? AS JSON) WHERE source_event_id = ?", [r#"{"not":"a segment list"}"#.into(), poison.as_str().into()])).await.expect("test must inject incompatible persisted segment JSON");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_artifact_derivations SET created_at = DATE_SUB(created_at, INTERVAL 1 SECOND) WHERE source_event_id = ?",
        [poison.as_str().into()],
    ))
    .await
    .expect("poison job must be the first claim candidate");
    let artifacts = ArtifactUseCase::new(build_mysql_artifact_store(db.clone()));
    artifacts
        .derive_pending(60, 2)
        .await
        .expect("derivation run must finish both jobs");
    assert_eq!(
        scalar_string(
            &db,
            "SELECT status AS value FROM secretary_artifact_derivations WHERE source_event_id = ?",
            vec![poison.as_str().into()]
        )
        .await,
        "failed"
    );
    assert_eq!(scalar_string(&db, "SELECT last_error_code AS value FROM secretary_artifact_derivations WHERE source_event_id = ?", vec![poison.as_str().into()]).await, "invalid_segments_json");
    assert_eq!(
        scalar_string(
            &db,
            "SELECT status AS value FROM secretary_artifact_derivations WHERE source_event_id = ?",
            vec![normal.as_str().into()]
        )
        .await,
        "completed"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_artifacts WHERE source_event_id = ?",
            vec![normal.as_str().into()]
        )
        .await,
        1
    );
}
