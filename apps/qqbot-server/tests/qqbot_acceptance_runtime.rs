//! QQBot 运行时验收测试。
//!
//! 这些测试必须走真实生产入口与隔离 MySQL，禁止用 Fake 冒充 L4/L5。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use personal_secretary::{
    ConnectionEpochId, ContentTrustLevel, ConversationKind, ConversationRef,
    InboundMessageEnvelope, IngestMessageOutcome, MessageSource, RecallCorrelationKey, RecallEvent,
    RecallEventId, RecallKind, SourceAccountRef, SourceMessageRef, VerifiedActor,
    VerifiedActorKind, build_mysql_inbound_event_store, build_mysql_recall_store,
};
use qqbot::napcat::{
    GroupMessageEvent, GroupRecallEvent, MessageSegment, NapCatEvent, NapCatEventHandler, RichKind,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use qqbot_server::config::{
    ArtifactConfig, DirectorySyncConfig, HealthConfig, IngestionConfig, RecallWalConfig,
};
use qqbot_server::production;

#[path = "../database/test_support/qqbot_migrations.rs"]
mod qqbot_migrations;

fn account(subject: &str) -> SourceAccountRef {
    SourceAccountRef::new(MessageSource::NapCat, subject.to_owned()).expect("valid account fixture")
}

fn recall_wal_config() -> RecallWalConfig {
    let nonce = Uuid::new_v4();
    let key_env = format!("QQBOT_TEST_RECALL_SPOOL_KEY_{}", nonce.simple());
    // The test uses a unique name to avoid racing other acceptance processes; no concurrent
    // environment mutation occurs before the server reads it.
    unsafe {
        std::env::set_var(
            &key_env,
            base64::engine::general_purpose::STANDARD.encode([7_u8; 32]),
        );
    }
    RecallWalConfig {
        path: std::env::temp_dir().join(format!("qqbot-recall-{nonce}.spool")),
        quarantine_dir: std::env::temp_dir().join(format!("qqbot-recall-{nonce}-quarantine")),
        key_env,
        ..RecallWalConfig::default()
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
    let db = sea_orm::Database::connect(url)
        .await
        .expect("connect isolated acceptance MySQL");
    qqbot_migrations::apply_qqbot_migrations(
        &db,
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("database/migrations"),
    )
    .await;
    db
}

async fn scalar_u64(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> u64 {
    let row = db
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
    u64::try_from(row).expect("acceptance count must not be negative")
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

/// 迁移只能在 `qqbot_accept_*` 隔离 schema 中验证；断言本版本的关键表与账户 fencing 字段。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_notification_policy_migration_creates_fenced_schema() {
    let db = isolated_db().await;
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_name IN \
             ('secretary_notification_policy_families', \
              'secretary_notification_policy_revisions', \
              'secretary_notification_candidates', \
              'secretary_notification_evaluation_requests', \
              'secretary_notification_decisions', \
              'secretary_notification_feedback')",
            Vec::new(),
        )
        .await,
        6,
        "通知策略迁移必须创建全部六张策略与反馈表",
    );
}

/// 账户策略 epoch 必须用无符号列，避免 SeaORM 将 MySQL `BIGINT UNSIGNED` 解码为 `i64`。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_migration_uses_unsigned_epoch() {
    let db = isolated_db().await;
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM information_schema.columns \
             WHERE table_schema = DATABASE() \
               AND table_name = 'secretary_accounts' \
               AND column_name = 'policy_epoch' \
               AND column_type = 'bigint unsigned'",
            Vec::new(),
        )
        .await,
        1,
        "账户策略 epoch 必须使用 BIGINT UNSIGNED",
    );
}

/// Family Head 必须引用同一 Family 的 Revision，防止跨策略族篡改当前生效策略。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_migration_fences_family_head_to_own_revision() {
    let db = isolated_db().await;
    let account_id = 9_000_000_u64 + (Uuid::new_v4().as_u128() % 1_000_000) as u64;
    let first_family_id = Uuid::new_v4().to_string();
    let second_family_id = Uuid::new_v4().to_string();
    let first_revision_id = Uuid::new_v4().to_string();
    let second_revision_id = Uuid::new_v4().to_string();

    for statement in [
        Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_accounts \
             (id, source_channel, platform_account_id, status, policy_epoch) \
             VALUES (?, 'napcat', ?, 'active', 0)",
            [
                account_id.into(),
                format!("family-head-{account_id}").into(),
            ],
        ),
        Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_notification_policy_families \
             (policy_family_id, account_id, canonical_scope_key, policy_kind, current_revision_id, generation) \
             VALUES (?, ?, ?, 'account_default', NULL, 1)",
            [
                first_family_id.clone().into(),
                account_id.into(),
                "first".into(),
            ],
        ),
        Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_notification_policy_families \
             (policy_family_id, account_id, canonical_scope_key, policy_kind, current_revision_id, generation) \
             VALUES (?, ?, ?, 'account_default', NULL, 1)",
            [
                second_family_id.clone().into(),
                account_id.into(),
                "second".into(),
            ],
        ),
        Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_notification_policy_revisions \
             (policy_revision_id, policy_family_id, revision_number, revision_kind, rule_json, audit_summary) \
             VALUES (?, ?, 1, 'rule', JSON_OBJECT(), 'first revision')",
            [
                first_revision_id.clone().into(),
                first_family_id.clone().into(),
            ],
        ),
        Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_notification_policy_revisions \
             (policy_revision_id, policy_family_id, revision_number, revision_kind, rule_json, audit_summary) \
             VALUES (?, ?, 1, 'rule', JSON_OBJECT(), 'second revision')",
            [
                second_revision_id.clone().into(),
                second_family_id.clone().into(),
            ],
        ),
    ] {
        db.execute_raw(statement)
            .await
            .expect("Family Head fixture must persist");
    }

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_notification_policy_families \
         SET current_revision_id = ? WHERE policy_family_id = ?",
        [
            first_revision_id.clone().into(),
            first_family_id.clone().into(),
        ],
    ))
    .await
    .expect("Family must accept its own revision as Head");
    assert_eq!(
        scalar_string(
            &db,
            "SELECT current_revision_id AS value FROM secretary_notification_policy_families \
             WHERE policy_family_id = ?",
            vec![first_family_id.clone().into()],
        )
        .await,
        first_revision_id,
        "成功提交后 Family Head 不得为空",
    );

    let cross_family_update = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_notification_policy_families \
             SET current_revision_id = ? WHERE policy_family_id = ?",
            [second_revision_id.into(), first_family_id.into()],
        ))
        .await;
    assert!(
        cross_family_update.is_err(),
        "MySQL 必须拒绝把另一 Family 的 Revision 写入当前 Family Head"
    );
}

/// 共用迁移加载器必须记录 Baseline；第二次加载不得重复执行任何 DDL。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_migration_loader_records_and_repeats_idempotently() {
    let db = isolated_db().await;
    let migrations_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("database/migrations");

    let recorded_before = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM qqbot_test_schema_migrations \
         WHERE migration_name = 'baseline:20260803_qqbot_schema_v1.sql'",
        Vec::new(),
    )
    .await;
    assert_eq!(recorded_before, 1, "全新 schema 必须记录 Baseline v1");

    qqbot_migrations::apply_qqbot_migrations(&db, &migrations_dir).await;

    let recorded_after = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM qqbot_test_schema_migrations \
         WHERE migration_name = 'baseline:20260803_qqbot_schema_v1.sql'",
        Vec::new(),
    )
    .await;
    assert_eq!(recorded_after, 1, "重复加载不得重复记录或执行 Baseline");
}

async fn drain_handle(handle: qqbot_server::worker_lifecycle::WorkerHandle) {
    assert!(
        handle.join_with_timeout(Duration::from_secs(10)).await,
        "production worker must stop within the acceptance deadline"
    );
}

/// L4：真实 RecallHandler + 真实有界 Worker + 真实 MySQL RecallStore，CHAR(36) 边界 + 幂等。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_realistic_recall_maps_to_stable_uuid_and_persists_idempotently() {
    let db = isolated_db().await;
    let account_subject = format!("accept-uuid-{}", Uuid::new_v4().simple());
    let acc = account(&account_subject);

    let recall_use_case = Arc::new(personal_secretary::RecallUseCase::new(
        build_mysql_recall_store(db.clone()),
    ));
    let (queue, worker) = production::spawn_recall_worker(recall_use_case, recall_wal_config())
        .expect("open recall WAL");
    let handler = production::RecallHandler::new(queue, acc.clone(), 1_839_717_811);

    let group_id: i64 = 671_260_344;
    let message_id = format!("msg-{}", Uuid::new_v4().simple());

    // 两次同 message_id：第一次写，第二次幂等。
    for time in [1_800_000_500_i64, 1_800_000_600] {
        handler
            .handle_group_recall(GroupRecallEvent {
                group_id,
                user_id: 9_000_001,
                operator_id: Some(9_000_001),
                message_id: message_id.clone(),
                time,
                raw_event: serde_json::json!({"notice_type": "group_recall"}),
            })
            .await
            .expect("handler must propagate queue success");
    }

    drop(handler);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let count = scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_recall_events \
                 WHERE account_id = (SELECT id FROM secretary_accounts WHERE platform_account_id = ?) \
                   AND correlation_key LIKE CONCAT('%', ?)",
                vec![account_subject.clone().into(), message_id.clone().into()],
            )
            .await;
            if count == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("recall worker must durably apply the queued callback before shutdown");
    drain_handle(worker.signal_and_detach()).await;

    let count = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_recall_events \
         WHERE account_id = (SELECT id FROM secretary_accounts WHERE platform_account_id = ?) \
           AND correlation_key LIKE CONCAT('%', ?)",
        vec![account_subject.into(), message_id.into()],
    )
    .await;
    assert_eq!(count, 1, "撤回事件必须按关联键幂等持久化到 MySQL");
}

/// L5：pending tombstone 跨进程重启可恢复 —— 全程真实 MySQL。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_pending_recall_survives_process_restart() {
    let db = isolated_db().await;
    let account_subject = format!("accept-restart-{}", Uuid::new_v4().simple());
    let acc = account(&account_subject);

    // 进程 1：只把撤回持久化到 inbox，不调用 tombstone 用例。
    let recall_use_case_1 =
        personal_secretary::RecallUseCase::new(build_mysql_recall_store(db.clone()));
    let message_id = format!("restart-{}", Uuid::new_v4().simple());
    let recall_event_id = RecallEventId::new(Uuid::new_v4().to_string()).expect("uuid id");
    recall_use_case_1
        .enqueue(&RecallEvent {
            recall_event_id: recall_event_id.clone(),
            account: acc.clone(),
            kind: RecallKind::Group,
            correlation: RecallCorrelationKey::new(
                acc.clone(),
                MessageSource::NapCat,
                ConversationRef::new(ConversationKind::Group, String::from("671260344"))
                    .expect("group"),
                message_id.clone(),
            )
            .expect("correlation key"),
            operator_platform_id: Some("op-1".into()),
            occurred_at_unix_secs: 1_800_001_000,
        })
        .await
        .expect("durable enqueue must succeed");
    drop(recall_use_case_1);

    assert_eq!(
        scalar_string(
            &db,
            "SELECT status AS value FROM secretary_recall_inbox WHERE recall_event_id = ?",
            vec![recall_event_id.as_str().into()],
        )
        .await,
        "pending"
    );

    // 进程 2：全新 use case 与生产 Worker 扫描同一 inbox。
    let recall_use_case_2 = Arc::new(personal_secretary::RecallUseCase::new(
        build_mysql_recall_store(db.clone()),
    ));
    let (_queue, worker) = production::spawn_recall_worker(recall_use_case_2, recall_wal_config())
        .expect("open recall WAL");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = scalar_string(
                &db,
                "SELECT status AS value FROM secretary_recall_inbox WHERE recall_event_id = ?",
                vec![recall_event_id.as_str().into()],
            )
            .await;
            if status == "applied" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("restarted Worker must apply durable recall");
    drain_handle(worker.signal_and_detach()).await;

    // 原消息随后入库，pending tombstone 在同一 MySQL 事务自动关联。
    let inbound_store = build_mysql_inbound_event_store(db.clone());
    let inbound_message = InboundMessageEnvelope::new(
        SourceMessageRef::new(
            MessageSource::NapCat,
            account_subject.clone(),
            message_id.clone(),
        )
        .expect("source ref"),
        ConversationRef::new(ConversationKind::Group, String::from("671260344")).expect("group"),
        VerifiedActor::new(VerifiedActorKind::External, "sender").expect("verified actor"),
        1_800_001_500,
        "restart message",
        Vec::new(),
    )
    .expect("inbound envelope");
    let outcome = inbound_store
        .insert_message_if_absent(&inbound_message)
        .await
        .expect("message ingest must succeed");
    let source_event_id = outcome.source_event_id().as_str().to_string();
    assert!(matches!(outcome, IngestMessageOutcome::Accepted { .. }));

    let status = scalar_string(
        &db,
        "SELECT status AS value FROM secretary_message_tombstones WHERE source_event_id = ?",
        vec![source_event_id.into()],
    )
    .await;
    assert_eq!(
        status, "applied",
        "重启后消息入库必须自动 applied pending tombstone"
    );
}

/// L5：MySQL 故障时回调只同步本地 WAL，普通消息仍可进入真实 ingestion Worker，并在恢复后转存。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_stalled_recall_store_does_not_block_websocket_and_retries() {
    let db = isolated_db().await;
    let account_subject = "1839717811".to_string();
    let acc = account(&account_subject);
    let recall_use_case = Arc::new(personal_secretary::RecallUseCase::new(
        build_mysql_recall_store(db.clone()),
    ));
    let mut wal = recall_wal_config();
    wal.drain_interval_ms = 25;
    let (queue, worker) =
        production::spawn_recall_worker(recall_use_case, wal).expect("open recall WAL");
    let recall_handler = Arc::new(production::RecallHandler::new(
        queue,
        acc.clone(),
        1_839_717_811,
    ));
    let message_id = format!("stall-{}", Uuid::new_v4().simple());

    db.execute_raw(Statement::from_string(
        DatabaseBackend::MySql,
        "RENAME TABLE secretary_recall_inbox TO secretary_recall_inbox_unavailable",
    ))
    .await
    .expect("failure injection must make inbox table unavailable");
    let inbound_store = build_mysql_inbound_event_store(db.clone());
    let epoch = inbound_store
        .begin_connection(&acc)
        .await
        .expect("begin epoch");
    inbound_store
        .mark_connection_connected(&epoch)
        .await
        .expect("connect epoch");
    let (ingestion, ingestion_worker) = production::spawn_ingestion_worker(
        inbound_store,
        epoch,
        IngestionConfig::default(),
        None,
        None,
        0,
        None,
    );
    let inbound: Arc<dyn NapCatEventHandler> =
        Arc::new(production::PersonalSecretaryInboundHandler {
            mapper: production::NapCatInboundMapper::new(1_839_717_811),
            queue: ingestion,
            group_whitelist: Arc::new(HashSet::new()),
            recall_handler: Some(Arc::clone(&recall_handler)),
        });

    let started = std::time::Instant::now();
    inbound
        .handle(NapCatEvent::GroupRecall(GroupRecallEvent {
            group_id: 671_260_344,
            user_id: 1,
            operator_id: Some(1),
            message_id: message_id.clone(),
            time: 1_800_002_000,
            raw_event: serde_json::json!({}),
        }))
        .await
        .expect("WAL append must acknowledge despite MySQL failure");
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "WebSocket callback must not wait for MySQL (actual {:?})",
        started.elapsed()
    );
    inbound
        .handle(NapCatEvent::GroupMessage(GroupMessageEvent {
            message_id: format!("ordinary-{}", Uuid::new_v4().simple()),
            group_id: 671_260_344,
            user_id: 2,
            raw_message: "ordinary".into(),
            normalized_text: "ordinary".into(),
            segments: Vec::new(),
            at_bot: false,
            time: 1_800_002_001,
            sender: None,
            is_self: false,
            raw_event: serde_json::Value::Null,
        }))
        .await
        .expect(
            "ordinary message must continue through callback while MySQL recall inbox is absent",
        );
    drop(inbound);
    let report = tokio::time::timeout(Duration::from_secs(10), ingestion_worker)
        .await
        .expect("ingestion timeout")
        .expect("ingestion worker panic");
    assert_eq!(report.accepted, 1);

    db.execute_raw(Statement::from_string(
        DatabaseBackend::MySql,
        "RENAME TABLE secretary_recall_inbox_unavailable TO secretary_recall_inbox",
    ))
    .await
    .expect("restoring MySQL must make recall inbox available again");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let count = scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_recall_events event \
                 JOIN secretary_accounts account ON account.id = event.account_id \
                 WHERE account.platform_account_id = ? AND event.platform_message_id = ?",
                vec![account_subject.clone().into(), message_id.clone().into()],
            )
            .await;
            if count == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("recovered MySQL must receive recall from local WAL");
    drain_handle(worker.signal_and_detach()).await;
}

/// L4：非白名单群的群撤回不入个人秘书数据库。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_non_whitelisted_group_recall_is_not_persisted() {
    let db = isolated_db().await;
    let acc = account("accept-whitelist-runtime");

    let recall_use_case = Arc::new(personal_secretary::RecallUseCase::new(
        build_mysql_recall_store(db.clone()),
    ));
    let (queue, worker) = production::spawn_recall_worker(recall_use_case, recall_wal_config())
        .expect("open recall WAL");
    let mut whitelist = HashSet::new();
    whitelist.insert(671_260_344_i64);
    let whitelist = Arc::new(whitelist);
    let handler = Arc::new(production::RecallHandler::new(queue, acc.clone(), 1));

    let inbound_handler: Arc<dyn NapCatEventHandler> =
        Arc::new(production::PersonalSecretaryInboundHandler {
            mapper: production::NapCatInboundMapper::new(1),
            queue: production::IngestionQueue::for_test(),
            group_whitelist: Arc::clone(&whitelist),
            recall_handler: Some(Arc::clone(&handler)),
        });

    inbound_handler
        .handle(NapCatEvent::GroupRecall(GroupRecallEvent {
            group_id: 99_999_999,
            user_id: 1,
            operator_id: Some(1),
            message_id: format!("not-whitelisted-{}", Uuid::new_v4().simple()),
            time: 1_800_003_000,
            raw_event: serde_json::json!({}),
        }))
        .await
        .expect("non-whitelisted group recall must be silently dropped, not errored");

    drop(inbound_handler);
    drop(handler);
    drain_handle(worker.signal_and_detach()).await;

    let count = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_recall_events \
         WHERE account_id = (SELECT id FROM secretary_accounts WHERE platform_account_id = ?)",
        vec![acc.account_id.clone().into()],
    )
    .await;
    assert_eq!(count, 0, "非白名单群撤回不得进入 MySQL");
}

/// L5：目录同步整体 deadline —— 真实 worker + 真实 hung source，超时返回 Timeout。
#[tokio::test]
#[ignore = "executed only by verify-qqbot-acceptance.ps1"]
async fn acceptance_directory_sync_honors_overall_deadline() {
    struct HungSource {
        hang_for: Duration,
        active: Arc<std::sync::atomic::AtomicUsize>,
        cancelled: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl HungSource {
        async fn hang(
            &self,
        ) -> Result<
            Vec<personal_secretary::DirectoryListEntry>,
            personal_secretary::DirectorySourceError,
        > {
            struct ActiveCall {
                active: Arc<std::sync::atomic::AtomicUsize>,
                cancelled: Arc<std::sync::atomic::AtomicUsize>,
                completed: bool,
            }
            impl Drop for ActiveCall {
                fn drop(&mut self) {
                    self.active
                        .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                    if !self.completed {
                        self.cancelled
                            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    }
                }
            }
            self.active
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let mut call = ActiveCall {
                active: Arc::clone(&self.active),
                cancelled: Arc::clone(&self.cancelled),
                completed: false,
            };
            tokio::time::sleep(self.hang_for).await;
            call.completed = true;
            Ok(Vec::new())
        }
    }
    #[async_trait::async_trait]
    impl personal_secretary::DirectorySourceT for HungSource {
        async fn list_friends(
            &self,
            _account: &personal_secretary::SourceAccountRef,
        ) -> Result<
            Vec<personal_secretary::DirectoryListEntry>,
            personal_secretary::DirectorySourceError,
        > {
            self.hang().await
        }
        async fn list_groups(
            &self,
            _account: &personal_secretary::SourceAccountRef,
        ) -> Result<
            Vec<personal_secretary::DirectoryListEntry>,
            personal_secretary::DirectorySourceError,
        > {
            self.hang().await
        }
        async fn list_recent_contacts(
            &self,
            _account: &personal_secretary::SourceAccountRef,
        ) -> Result<
            Vec<personal_secretary::DirectoryListEntry>,
            personal_secretary::DirectorySourceError,
        > {
            self.hang().await
        }
    }

    let db = isolated_db().await;
    let acc = account("accept-deadline");
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&acc)
        .await
        .expect("directory acceptance account must exist");
    let store: Arc<dyn personal_secretary::DirectoryStoreT> =
        personal_secretary::build_mysql_directory_store(db);
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cancelled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let source: Arc<dyn personal_secretary::DirectorySourceT> = Arc::new(HungSource {
        hang_for: Duration::from_secs(10),
        active: Arc::clone(&active),
        cancelled: Arc::clone(&cancelled),
    });
    let budget = personal_secretary::DirectorySyncBudget {
        snapshot_ttl_secs: 3600,
        sync_deadline_secs: 1,
        max_entries: 100,
        retry_initial_ms: 1,
        retry_max_ms: 1,
    };
    let use_case = Arc::new(
        personal_secretary::DirectorySyncUseCase::new(source, store, budget)
            .expect("use case must build"),
    );
    let config = DirectorySyncConfig {
        enabled: true,
        snapshot_ttl_secs: 3600,
        sync_deadline_secs: 1,
        max_entries: 100,
        scan_interval_ms: 60_000,
        retry_initial_ms: 1,
        retry_max_ms: 1,
    };
    let handle = production::spawn_directory_sync_worker(use_case, acc, config);

    tokio::time::timeout(Duration::from_secs(3), async {
        while cancelled.load(std::sync::atomic::Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("deadline must cancel at least one hung source future");
    drain_handle(handle.signal_and_detach()).await;
    assert_eq!(
        active.load(std::sync::atomic::Ordering::Acquire),
        0,
        "shutdown must leave no detached source calls"
    );
}

/// L4：生产 health Worker 从 MySQL 恢复 Gap 状态，并通过 reader 发布快照。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_runtime_health_snapshot_reports_real_subsystems() {
    use personal_secretary::{HealthStatus, SubsystemHealth};
    let db = isolated_db().await;
    let acc = account(&format!("health-{}", Uuid::new_v4().simple()));
    let inbound = build_mysql_inbound_event_store(db.clone());
    let epoch = inbound.begin_connection(&acc).await.expect("begin epoch");
    inbound
        .mark_connection_connected(&epoch)
        .await
        .expect("connect epoch");
    inbound
        .mark_connection_uncertain(
            &epoch,
            personal_secretary::IngestionGapReason::QueueOverflow,
        )
        .await
        .expect("persist uncertain Gap");

    let state = production::RuntimeHealthState::new();
    let recall_spool_telemetry = production::RecallSpoolTelemetry::new(1_024);
    state.set_websocket_connected(true);
    let aggregator = Arc::new(
        production::build_runtime_health_aggregator_with_recall_spool(
            Arc::clone(&state),
            recall_spool_telemetry,
            1,
            300,
        ),
    );
    let config = HealthConfig {
        enabled: true,
        cache_ttl_secs: 1,
        log_interval_ms: 1_000,
        worker_success_stale_secs: 300,
    };
    let (mut reader, handle) =
        production::spawn_health_log_worker(aggregator, state, db, acc, config);
    let snapshot = tokio::time::timeout(Duration::from_secs(10), reader.changed())
        .await
        .expect("health sample timeout")
        .expect("health publisher closed");
    drain_handle(handle.signal_and_detach()).await;
    assert_eq!(snapshot.subsystems.len(), 5);
    let ws: &SubsystemHealth = snapshot
        .subsystems
        .iter()
        .find(|s| s.name == "websocket")
        .expect("websocket producer");
    assert_eq!(ws.status, HealthStatus::Healthy);
    let history: &SubsystemHealth = snapshot
        .subsystems
        .iter()
        .find(|s| s.name == "history_completeness")
        .expect("history producer");
    assert_eq!(history.status, HealthStatus::Degraded);
    assert_eq!(
        history.last_error.as_deref(),
        Some("uncertain_gaps_present")
    );
    let mysql = snapshot
        .subsystems
        .iter()
        .find(|s| s.name == "mysql")
        .expect("mysql producer");
    assert_eq!(mysql.status, HealthStatus::Healthy);
    let recall_spool = snapshot
        .subsystems
        .iter()
        .find(|s| s.name == "recall_spool")
        .expect("recall spool producer");
    assert_eq!(recall_spool.status, HealthStatus::Uncertain);
    assert_eq!(recall_spool.metrics.get("backlog"), Some(&0));
    assert_ne!(reader.latest().overall_status, HealthStatus::Healthy);
}

/// L4：生产 health Worker 的实际 tracing 输出只包含允许列表 reason code。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_health_logs_are_structured_and_redacted() {
    #[derive(Clone)]
    struct BufferWriter(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for BufferWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let db = isolated_db().await;
    let injected = "https://secret.example/?access_token=hidden Bearer 123456789";
    let acc = account(injected);
    let state = production::RuntimeHealthState::new();
    let recall_spool_telemetry = production::RecallSpoolTelemetry::new(1_024);
    state.set_websocket_connected(false);
    let aggregator = Arc::new(
        production::build_runtime_health_aggregator_with_recall_spool(
            Arc::clone(&state),
            recall_spool_telemetry,
            1,
            300,
        ),
    );
    let bytes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer_bytes = Arc::clone(&bytes);
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || BufferWriter(Arc::clone(&writer_bytes)))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let (mut reader, handle) = production::spawn_health_log_worker(
        aggregator,
        state,
        db,
        acc,
        HealthConfig {
            enabled: true,
            cache_ttl_secs: 1,
            log_interval_ms: 1_000,
            worker_success_stale_secs: 300,
        },
    );
    tokio::time::timeout(Duration::from_secs(10), reader.changed())
        .await
        .expect("health log sample timeout")
        .expect("health publisher closed");
    drain_handle(handle.signal_and_detach()).await;
    let output = String::from_utf8(bytes.lock().expect("log buffer lock").clone())
        .expect("tracing output must be UTF-8");
    assert!(output.contains("runtime health snapshot"));
    assert!(output.contains("reason_code"));
    for forbidden in [
        injected,
        "access_token",
        "Bearer ",
        "https://secret.example",
        "123456789",
    ] {
        assert!(
            !output.contains(forbidden),
            "health tracing output leaked forbidden value {forbidden}: {output}"
        );
    }
}

/// L4：NapCat 富消息经过 mapper、生产 ingestion Worker、消息事务和 Artifact Worker 落库。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_rich_message_ingestion_creates_bounded_artifacts() {
    let db = isolated_db().await;
    let self_qq_id = 1_000_000_000_i64
        + i64::from_str_radix(&Uuid::new_v4().simple().to_string()[..8], 16).expect("hex UUID");
    let account_subject = self_qq_id.to_string();
    let acc = account(&account_subject);
    let store = build_mysql_inbound_event_store(db.clone());
    let epoch = store.begin_connection(&acc).await.expect("begin epoch");
    store
        .mark_connection_connected(&epoch)
        .await
        .expect("connect epoch");
    let artifact_use_case = Arc::new(personal_secretary::ArtifactUseCase::new(
        personal_secretary::build_mysql_artifact_store(db.clone()),
    ));
    let (queue, ingestion) = production::spawn_ingestion_worker(
        store,
        epoch,
        IngestionConfig::default(),
        None,
        Some(artifact_use_case.clone()),
        3600,
        None,
    );
    let message_id = format!("artifact-{}", Uuid::new_v4().simple());
    let mapped = production::NapCatInboundMapper::new(self_qq_id)
        .map_group(GroupMessageEvent {
            message_id: message_id.clone(),
            group_id: 671_260_344,
            user_id: 42,
            raw_message: String::new(),
            normalized_text: "rich".into(),
            segments: vec![
                MessageSegment::Image {
                    file: "image-ref".into(),
                    url: Some("https://secret.example/signed?token=hidden".into()),
                },
                MessageSegment::Forward {
                    id: "forward-ref".into(),
                },
                MessageSegment::Rich {
                    kind: RichKind::Json,
                    data: Some("{\"private\":\"payload\"}".into()),
                    summary: Some("bounded summary".into()),
                },
            ],
            at_bot: false,
            time: 1_800_006_000,
            sender: None,
            is_self: false,
            raw_event: serde_json::Value::Null,
        })
        .expect("production mapper must accept rich message");
    queue.try_enqueue(mapped).expect("ingestion enqueue");
    drop(queue);
    let report = tokio::time::timeout(Duration::from_secs(10), ingestion)
        .await
        .expect("ingestion worker timeout")
        .expect("ingestion worker panic");
    assert_eq!(report.accepted, 1);

    let config = ArtifactConfig {
        ttl_scan_interval_ms: 1_000,
        ..ArtifactConfig::default()
    };
    let handle = production::spawn_artifact_ttl_worker(artifact_use_case, config);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let count = scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_artifacts artifact \
                 JOIN secretary_source_events event ON event.source_event_id = artifact.source_event_id \
                 WHERE event.platform_event_id = ?",
                vec![message_id.clone().into()],
            )
            .await;
            if count == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("Artifact Worker must derive all typed segments");
    drain_handle(handle.signal_and_detach()).await;

    let stored_segments = scalar_string(
        &db,
        "SELECT CAST(content.segments AS CHAR) AS value FROM secretary_message_contents content \
         JOIN secretary_source_events event ON event.source_event_id = content.source_event_id \
         WHERE event.platform_event_id = ?",
        vec![message_id.into()],
    )
    .await;
    assert!(!stored_segments.contains("secret.example"));
    assert!(!stored_segments.contains("private"));
    assert!(stored_segments.contains("forward"));
    assert!(stored_segments.contains("rich"));
}

/// L5：消息事务留下 durable 派生任务，Artifact Worker 跨重启派生并保持 TTL 状态。
#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_artifact_ttl_worker_and_content_policy_survive_restart() {
    let db = isolated_db().await;
    let account_subject = format!("accept-ttl-{}", Uuid::new_v4().simple());
    let inbound = build_mysql_inbound_event_store(db.clone());
    let message_id = format!("ttl-{}", Uuid::new_v4().simple());
    let message = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &account_subject, &message_id)
            .expect("source ref"),
        ConversationRef::new(ConversationKind::Group, "671260344").expect("group"),
        VerifiedActor::new(VerifiedActorKind::External, "sender").expect("actor"),
        1,
        String::new(),
        vec![personal_secretary::ContentSegment::Media {
            kind: personal_secretary::MediaKind::Image,
            source_key: "expired-image".into(),
            source_url: None,
            display_name: None,
        }],
    )
    .expect("message");
    let source_event_id = inbound
        .insert_message_if_absent(&message)
        .await
        .expect("message transaction")
        .source_event_id()
        .clone();
    assert_eq!(
        scalar_string(
            &db,
            "SELECT status AS value FROM secretary_artifact_derivations WHERE source_event_id = ?",
            vec![source_event_id.as_str().into()],
        )
        .await,
        "pending"
    );

    let config = ArtifactConfig {
        default_ttl_secs: 1,
        ttl_scan_interval_ms: 1_000,
        ..ArtifactConfig::default()
    };
    let use_case_1 = Arc::new(personal_secretary::ArtifactUseCase::new(
        personal_secretary::build_mysql_artifact_store(db.clone()),
    ));
    let worker_1 = production::spawn_artifact_ttl_worker(use_case_1, config.clone());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = db
                .query_one_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "SELECT availability AS value FROM secretary_artifacts WHERE source_event_id = ?",
                    [source_event_id.as_str().into()],
                ))
                .await
                .expect("artifact status query")
                .and_then(|row| row.try_get::<String>("", "value").ok());
            if status.as_deref() == Some("expired") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("Artifact Worker must derive and expire the old message");
    drain_handle(worker_1.signal_and_detach()).await;

    let use_case_2 = Arc::new(personal_secretary::ArtifactUseCase::new(
        personal_secretary::build_mysql_artifact_store(db.clone()),
    ));
    let worker_2 = production::spawn_artifact_ttl_worker(use_case_2, config);
    tokio::time::sleep(Duration::from_millis(100)).await;
    drain_handle(worker_2.signal_and_detach()).await;
    assert_eq!(
        scalar_string(
            &db,
            "SELECT availability AS value FROM secretary_artifacts WHERE source_event_id = ?",
            vec![source_event_id.as_str().into()],
        )
        .await,
        "expired",
        "restart must not make an expired Artifact available again"
    );
    assert!(
        personal_secretary::ArtifactEnvelope::new(
            personal_secretary::ArtifactId::new(Uuid::new_v4().to_string()).expect("artifact id"),
            account(&account_subject),
            personal_secretary::SourceEventId::new(Uuid::new_v4().to_string()).expect("source id"),
            ConversationRef::new(ConversationKind::Group, "671260344").expect("group"),
            personal_secretary::ArtifactKind::Image,
            "no-retention",
            ContentTrustLevel::NeverLongTerm,
            1,
            None,
        )
        .is_err(),
        "NeverLongTerm must not create a persistent Artifact even without a TTL"
    );
}

#[allow(dead_code)]
fn _ensure_imports_used() {
    let _ = ConnectionEpochId::new("test");
    let _ = IngestionConfig::default();
    let _ = HealthConfig::default();
}
