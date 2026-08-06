//! EVT-007-MSG 延迟 Reply 解析的隔离 MySQL 聚焦测试。
//!
//! 需要 QQBOT_TEST_DATABASE_URL 指向隔离的 MySQL schema（`qqbot_accept_` 前缀）；
//! 默认 #[ignore]。派生 schema 随机命名且测试结束时精确清理；清理失败必须报告。
//!
//! 验证：子事件先于父事件到达时，待解析状态随 SourceEvent 在同一事务持久化，
//! 父事件随后经实时/回补/Duplicate 重放到达时完成幂等解析；后台修复 Worker
//! 按租约/fencing/指数退避重试 unresolved 候选（无需消息重放）；跨重启安全；
//! 跨账号/跨会话同名平台消息 ID fail-closed；事务失败零半提交且可恢复；
//! 并发父子处理最终恰好一条正式关系；父永久缺失保持 unresolved；
//! 延迟关系建立后线程投影最终修复；投影领取后提交前解析必须 LeaseLost；
//! 提交后自愈失败不破坏已提交契约且 reconcile 可恢复；
//! Reply 解析删除事件所有出边（含非确定性边）；终态空线程撤销语义派生但不写
//! 虚假历史；投影计划遇终态目标整体失败；退避簿 fencing 防旧 Worker 覆盖。

mod common;

use common::{isolated_db, scalar_string, scalar_u64};
use personal_secretary::{
    ClaimKind, ContentSegment, ConversationKind, ConversationRef, DeterministicThreadPlanner,
    DeterministicThreadPolicy, InboundMessageEnvelope, MediaKind, MessageSource, RichContentKind,
    SourceMessageRef, ThreadClaimCandidate, ThreadClaimId, ThreadProjectionUseCase,
    ThreadSemanticPatch, VerifiedActor, VerifiedActorKind,
};
use personal_secretary_mysql::{
    build_mysql_backfill_store, build_mysql_inbound_event_store, build_mysql_reply_reconcile_store,
    build_mysql_thread_projection_store, build_mysql_thread_semantic_store,
};
use sea_orm::ConnectionTrait;
use std::sync::Arc;

/// 场景包装：tokio::spawn 确保 panic 后派生 schema 必然在 finally 删除。
/// 清理失败打印到 stderr（报告而不吞掉），场景结果原样返回。
async fn run_scenario<F>(suffix: &str, scenario: impl FnOnce(sea_orm::DatabaseConnection) -> F)
where
    F: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let (db, schema) = isolated_db(suffix).await;
    let outcome = tokio::spawn(scenario(db.clone())).await;
    // finally：无论场景成功、失败还是 panic，都先删除派生 schema。
    let cleanup = db
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS `{schema}`"))
        .await;
    if let Err(error) = cleanup {
        eprintln!("schema cleanup failed for {schema}: {error}");
    }
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(message)) => panic!("scenario must pass: {message}"),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
}

/// 最小合法入站信封；`reply_to` 为 None 时不带 Reply 段。
fn envelope(
    account_id: &str,
    message_id: &str,
    conv_kind: ConversationKind,
    conv_id: &str,
    actor_id: &str,
    occurred_at_unix_secs: i64,
    reply_to: Option<&str>,
) -> InboundMessageEnvelope {
    let segments = reply_to
        .map(|parent| {
            vec![ContentSegment::Reply {
                platform_message_id: parent.into(),
            }]
        })
        .unwrap_or_default();
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, account_id, message_id).unwrap(),
        ConversationRef::new(conv_kind, conv_id).unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, actor_id).unwrap(),
        occurred_at_unix_secs,
        format!("message text for {message_id}"),
        segments,
    )
    .unwrap()
}

/// NapCat 实测群文件父消息：可引用 ID 属于历史中的 `file` 消息，不属于 `group_upload`
/// notice 的 file.id。只保留有界文件元数据，与生产历史适配器映射一致。
fn file_envelope(
    account_id: &str,
    message_id: &str,
    conv_id: &str,
    actor_id: &str,
    occurred_at_unix_secs: i64,
) -> InboundMessageEnvelope {
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, account_id, message_id).unwrap(),
        ConversationRef::new(ConversationKind::Group, conv_id).unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, actor_id).unwrap(),
        occurred_at_unix_secs,
        String::new(),
        vec![ContentSegment::Media {
            kind: MediaKind::File,
            source_key: "napcat-file-key".into(),
            source_url: None,
            display_name: Some("sample.txt".into()),
        }],
    )
    .unwrap()
}

/// NapCat 实测 Ark/JSON 卡片是历史中拥有稳定 message_id 的普通消息，不属于 notice。
fn rich_card_envelope(
    account_id: &str,
    message_id: &str,
    conv_id: &str,
    actor_id: &str,
    occurred_at_unix_secs: i64,
) -> InboundMessageEnvelope {
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, account_id, message_id).unwrap(),
        ConversationRef::new(ConversationKind::Group, conv_id).unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, actor_id).unwrap(),
        occurred_at_unix_secs,
        String::new(),
        vec![ContentSegment::Rich {
            kind: RichContentKind::Json,
            source_key: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .into(),
            summary: Some("[卡片消息]".into()),
        }],
    )
    .unwrap()
}

/// 隔离 schema 上的真实线程投影用例（与生产 Worker 相同的 store + planner）。
fn projection_use_case(db: sea_orm::DatabaseConnection) -> ThreadProjectionUseCase {
    ThreadProjectionUseCase::new(
        build_mysql_thread_projection_store(db),
        DeterministicThreadPlanner::new(DeterministicThreadPolicy::new(3600).unwrap()),
        100,
        60,
        3600,
    )
    .unwrap()
}

/// 读取事件的 reply_to_event_id（None 表示仍 pending/无父）。
async fn resolved_parent_of(
    db: &sea_orm::DatabaseConnection,
    source_event_id: &str,
) -> Option<String> {
    let row = db
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "SELECT reply_to_event_id AS value \
             FROM secretary_source_events WHERE source_event_id = ?",
            vec![source_event_id.into()],
        ))
        .await
        .expect("query reply state")
        .expect("event must exist");
    row.try_get::<Option<String>>("", "value").expect("decode")
}

/// 读取事件的线程（secretary_thread_events 成员投影）。
async fn thread_of(db: &sea_orm::DatabaseConnection, source_event_id: &str) -> Option<String> {
    let row = db
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "SELECT thread_id AS value FROM secretary_thread_events \
             WHERE source_event_id = ?",
            vec![source_event_id.into()],
        ))
        .await
        .ok()
        .flatten();
    row.and_then(|row| row.try_get::<Option<String>>("", "value").ok().flatten())
}

/// 事件是否处于 unresolved pending（引用父平台消息 ID 但尚未解析）。
async fn is_pending(db: &sea_orm::DatabaseConnection, source_event_id: &str) -> bool {
    scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_source_events \
         WHERE source_event_id = ? AND reply_to_platform_event_id IS NOT NULL \
           AND reply_to_event_id IS NULL",
        vec![source_event_id.into()],
    )
    .await
        > 0
}

/// Reply 正式关系边计数（from=子, to=父, kind=reply）。
async fn reply_relation_count(
    db: &sea_orm::DatabaseConnection,
    child_id: &str,
    parent_id: &str,
) -> u64 {
    scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_thread_relations \
         WHERE from_event_id = ? AND to_event_id = ? AND relation_kind = 'reply'",
        vec![child_id.into(), parent_id.into()],
    )
    .await
}

/// 线程当前状态（secretary_event_threads.status，None 表示线程行不存在）。
async fn thread_status(db: &sea_orm::DatabaseConnection, thread_id: &str) -> Option<String> {
    let row = db
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "SELECT status AS value FROM secretary_event_threads WHERE thread_id = ?",
            vec![thread_id.into()],
        ))
        .await
        .ok()
        .flatten();
    row.and_then(|row| row.try_get::<Option<String>>("", "value").ok().flatten())
}

/// 账号下 open/reopened 线程计数（与 retriever 的 Owner 状态统计一致）。
async fn open_thread_count(db: &sea_orm::DatabaseConnection, account: &str) -> u64 {
    scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_event_threads t \
         JOIN secretary_accounts a ON a.id = t.account_id \
         WHERE a.platform_account_id = ? AND t.status IN ('open', 'reopened')",
        vec![account.into()],
    )
    .await
}

/// 线程语义批处理状态行数（无则 0）。
async fn semantic_state_count(db: &sea_orm::DatabaseConnection, thread_id: &str) -> u64 {
    scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_thread_semantic_state WHERE thread_id = ?",
        vec![thread_id.into()],
    )
    .await
}

/// 事件是否仍有投影租约（claims）行。
async fn projection_claim_exists(db: &sea_orm::DatabaseConnection, source_event_id: &str) -> bool {
    scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_thread_projection_claims \
         WHERE source_event_id = ?",
        vec![source_event_id.into()],
    )
    .await
        > 0
}

/// 线程是否有一条 system_recovery 权威的状态历史记录。
async fn has_system_recovery_history(db: &sea_orm::DatabaseConnection, thread_id: &str) -> bool {
    scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM secretary_thread_status_history \
         WHERE thread_id = ? AND authority = 'system_recovery'",
        vec![thread_id.into()],
    )
    .await
        > 0
}

/// 线程 root 事件（审计元数据）。
async fn thread_root_event(db: &sea_orm::DatabaseConnection, thread_id: &str) -> Option<String> {
    let row = db
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "SELECT root_event_id AS value FROM secretary_event_threads WHERE thread_id = ?",
            vec![thread_id.into()],
        ))
        .await
        .ok()
        .flatten();
    row.and_then(|row| row.try_get::<Option<String>>("", "value").ok().flatten())
}

async fn reply_reconcile_migration_record_count(db: &sea_orm::DatabaseConnection) -> u64 {
    scalar_u64(
        db,
        "SELECT COUNT(*) AS value FROM qqbot_test_schema_migrations \
         WHERE migration_name = '20260804_qqbot_reply_reconcile.sql'",
        Vec::new(),
    )
    .await
}

fn assert_migration_schema_error(error: &str, statement: usize, dimension: &str) {
    assert!(
        error.contains(&format!("statement {statement}"))
            && (error.contains("1242") || error.contains("Subquery returns more than 1 row")),
        "负向迁移必须在{dimension}复验处以多行标量子查询错误 fail-closed，实际: {error}"
    );
}

// ── 1. 父先子后：即时关联 ────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn parent_first_immediate_reply() {
    run_scenario("_evt007s1", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());

        let parent = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-1",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e}"))?
            .source_event_id()
            .clone();
        let child = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-1",
                ConversationKind::Group,
                "g-1",
                "bob",
                101,
                Some("p-1"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?;
        let child_id = child.source_event_id().clone();

        // 父已存在：子事件直接携带正式关系，无需 pending。
        let resolved = resolved_parent_of(&db, child_id.as_str()).await;
        assert_eq!(
            resolved.as_deref(),
            Some(parent.as_str()),
            "父先到时子事件必须即时关联正式父事件"
        );
        assert!(
            !is_pending(&db, child_id.as_str()).await,
            "不得残留 pending"
        );
        Ok(())
    })
    .await;
}

// ── 2. 子先父后（同进程）+ 线程投影最终修复 ─────────────────────────────

#[tokio::test]
#[ignore]
async fn child_first_parent_arrives_and_projection_repairs() {
    run_scenario("_evt007s2", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let projection = projection_use_case(db.clone());

        // 子先到：pending 随 SourceEvent 同一事务持久化。
        let child_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-2",
                ConversationKind::Group,
                "g-1",
                "bob",
                101,
                Some("p-2"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();
        assert!(
            is_pending(&db, child_id.as_str()).await,
            "父缺失时子事件必须持久化为 unresolved"
        );
        assert!(
            resolved_parent_of(&db, child_id.as_str()).await.is_none(),
            "父缺失时不得猜测父事件"
        );

        // 子先被线程投影：进入自己的线程，无 Reply 边。
        let run = projection
            .run_once()
            .await
            .map_err(|e| format!("project child failed: {e}"))?
            .expect("child must be projected");
        assert_eq!(run.events_projected, 1);
        let child_thread_before = thread_of(&db, child_id.as_str())
            .await
            .expect("child must be in a thread");
        // 模拟语义 Worker 已对旧线程建立语义批处理状态（含活跃租约），
        // 解析后必须被清除，避免残留租约/游标悬挂在空线程上。
        db.execute_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "INSERT INTO secretary_thread_semantic_state \
             (thread_id, last_source_event_id, lease_token, lease_expires_at, attempts, updated_at) \
             VALUES (?, ?, 'lease-before-resolution', UTC_TIMESTAMP(6) + INTERVAL 60 SECOND, 1, UTC_TIMESTAMP(6))",
            vec![
                child_thread_before.clone().into(),
                child_id.as_str().into(),
            ],
        ))
        .await
        .map_err(|e| format!("seed semantic state failed: {e}"))?;
        assert_eq!(
            semantic_state_count(&db, &child_thread_before).await,
            1,
            "前置：语义状态必须已注入"
        );

        // 父后到：同进程内父事务回填子事件为正式关系，并失效旧线程投影。
        let parent_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-2",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e:?}"))?
            .source_event_id()
            .clone();
        assert_eq!(
            resolved_parent_of(&db, child_id.as_str()).await.as_deref(),
            Some(parent_id.as_str()),
            "父后到必须把 pending 子事件解析为正式关系"
        );
        assert!(!is_pending(&db, child_id.as_str()).await);
        assert!(
            thread_of(&db, child_id.as_str()).await.is_none(),
            "旧线程成员必须被失效，等待重新投影"
        );

        // 复核修复断言：旧线程必须 closed（system_recovery 审计），语义状态与
        // 租约清除，root/latest 元数据保留；不得留下幽灵 open 线程。
        assert_eq!(
            thread_status(&db, &child_thread_before).await.as_deref(),
            Some("closed"),
            "子事件离开后空旧线程必须标记 closed"
        );
        assert!(
            has_system_recovery_history(&db, &child_thread_before).await,
            "空线程关闭必须写入 system_recovery 状态历史"
        );
        assert_eq!(
            semantic_state_count(&db, &child_thread_before).await,
            0,
            "空线程的语义批处理状态与租约必须清除"
        );
        assert_eq!(
            thread_root_event(&db, &child_thread_before).await.as_deref(),
            Some(child_id.as_str()),
            "closed 线程的 root 元数据保留（审计）"
        );
        assert!(
            !projection_claim_exists(&db, child_id.as_str()).await,
            "解析后不得残留投影租约"
        );
        assert_eq!(
            open_thread_count(&db, "acc-a").await,
            0,
            "解析后不得有幽灵 open 线程计入 Owner 状态"
        );

        // 重新投影：父子最终同线程，且存在真实 Reply 边（from=子, to=父）。
        // 父事件先被投影（新建线程），子随后以 reply_parent_thread_id 加入。
        projection
            .run_once()
            .await
            .map_err(|e| format!("project parent failed: {e}"))?;
        projection
            .run_once()
            .await
            .map_err(|e| format!("reproject child failed: {e}"))?;
        let child_thread = thread_of(&db, child_id.as_str())
            .await
            .expect("child must be reprojected");
        let parent_thread = thread_of(&db, parent_id.as_str())
            .await
            .expect("parent must be projected");
        assert_eq!(
            child_thread, parent_thread,
            "延迟解析后子事件必须进入父事件线程"
        );
        assert_ne!(
            child_thread, child_thread_before,
            "子事件必须离开父缺失时的旧线程"
        );
        assert_eq!(
            reply_relation_count(&db, child_id.as_str(), parent_id.as_str()).await,
            1,
            "最终线程关系必须使用真实 Reply 父事件"
        );
        assert_eq!(
            open_thread_count(&db, "acc-a").await,
            1,
            "重新投影后只有父线程计入 open 状态，closed 旧线程不计"
        );
        Ok(())
    })
    .await;
}

// ── 3. 子到达后跨重启（重建 store），父经实时/回补统一入口到达 ──────────

#[tokio::test]
#[ignore]
async fn restart_then_parent_arrives_through_shared_ingest_entry() {
    run_scenario("_evt007s3", |db| async move {
        // 第一次"运行"：子先到，pending 落库。
        let child_id = {
            let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
                build_mysql_inbound_event_store(db.clone());
            store
                .insert_message_if_absent(&envelope(
                    "acc-a",
                    "c-3",
                    ConversationKind::Group,
                    "g-1",
                    "bob",
                    101,
                    Some("p-3"),
                ))
                .await
                .map_err(|e| format!("insert child failed: {e}"))?
                .source_event_id()
                .clone()
        };
        assert!(is_pending(&db, child_id.as_str()).await);

        // 模拟重启：旧 store 销毁，仅依赖数据库持久化状态。
        // Backfill 与实时共享同一个统一幂等入口（BackfillGapUseCase 逐条调用
        // insert_message_if_absent），此处用同一入口代表父经回补路径到达。
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let parent_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-3",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent after restart failed: {e}"))?
            .source_event_id()
            .clone();

        assert_eq!(
            resolved_parent_of(&db, child_id.as_str()).await.as_deref(),
            Some(parent_id.as_str()),
            "重启后父到达仍必须完成解析"
        );
        assert!(!is_pending(&db, child_id.as_str()).await);
        Ok(())
    })
    .await;
}

// ── 4. Duplicate 子/父重放：不新增任何状态，且重放父修复 pending ─────────

#[tokio::test]
#[ignore]
async fn duplicate_replay_adds_nothing_and_repairs_pending() {
    run_scenario("_evt007s4", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());

        // 子先到（pending）。
        let child_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-4",
                ConversationKind::Group,
                "g-1",
                "bob",
                101,
                Some("p-4"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();
        // 父到达并完成解析。
        let parent_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-4",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e}"))?
            .source_event_id()
            .clone();
        assert_eq!(
            resolved_parent_of(&db, child_id.as_str()).await.as_deref(),
            Some(parent_id.as_str())
        );

        let events_before = scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_source_events",
            Vec::new(),
        )
        .await;

        // Duplicate 子重放：不新增 SourceEvent、不新增 pending、不重复关系。
        let replayed_child = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-4",
                ConversationKind::Group,
                "g-1",
                "bob",
                101,
                Some("p-4"),
            ))
            .await
            .map_err(|e| format!("replay child failed: {e}"))?;
        assert!(
            matches!(
                replayed_child,
                personal_secretary::IngestMessageOutcome::Duplicate { .. }
            ),
            "子重放必须返回 Duplicate"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events",
                Vec::new()
            )
            .await,
            events_before,
            "Duplicate 子不得新增 SourceEvent"
        );
        assert!(
            !is_pending(&db, child_id.as_str()).await,
            "Duplicate 子不得新增 pending"
        );

        // 遗留 pending 子（模拟父已存在但早期缺陷/交错残留未解析的状态）：
        // 直接写入数据库，随后 Duplicate 父重放必须修复它。
        let acc_a_id = scalar_u64(
            &db,
            "SELECT id AS value FROM secretary_accounts WHERE platform_account_id = 'acc-a'",
            Vec::new(),
        )
        .await;
        let g1_id = scalar_u64(
            &db,
            "SELECT c.id AS value FROM secretary_conversations c \
             JOIN secretary_accounts a ON a.id = c.account_id \
             WHERE a.platform_account_id = 'acc-a' AND c.platform_conversation_id = 'g-1'",
            Vec::new(),
        )
        .await;
        db.execute_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "INSERT INTO secretary_source_events \
             (source_event_id, account_id, conversation_id, source_channel, platform_event_id, \
              event_type, actor_platform_id, actor_kind, message_role, occurred_at_unix_secs, \
              reply_to_platform_event_id, reply_to_event_id, processing_status, received_at, created_at) \
             VALUES (UUID(), ?, ?, 'napcat', 'c-4b', 'message', 'bob', 'external', \
                     'external_observation', 102, 'p-4', NULL, 'pending', NOW(6), NOW(6))",
            vec![acc_a_id.into(), g1_id.into()],
        ))
        .await
        .map_err(|e| format!("seed legacy pending child failed: {e}"))?;
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events \
                 WHERE platform_event_id = 'c-4b'",
                Vec::new(),
            )
            .await,
            1,
            "遗留 pending 子必须就位"
        );

        let replayed_parent = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-4",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("replay parent failed: {e}"))?;
        assert!(
            matches!(
                replayed_parent,
                personal_secretary::IngestMessageOutcome::Duplicate { .. }
            ),
            "父重放必须返回 Duplicate"
        );
        let late_child_id = scalar_string(
            &db,
            "SELECT source_event_id AS value FROM secretary_source_events \
             WHERE platform_event_id = 'c-4b'",
            Vec::new(),
        )
        .await;
        assert_eq!(
            resolved_parent_of(&db, &late_child_id).await.as_deref(),
            Some(parent_id.as_str()),
            "Duplicate 父重放必须修复此前未完成的待解析关系"
        );
        assert!(!is_pending(&db, &late_child_id).await);
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events",
                Vec::new()
            )
            .await,
            events_before + 1,
            "重放不得新增 SourceEvent（仅允许 late_child 一次）"
        );
        Ok(())
    })
    .await;
}

// ── 5. 同平台消息 ID 跨账号/跨会话绝不串联（fail-closed）────────────────

#[tokio::test]
#[ignore]
async fn same_platform_message_id_never_crosses_account_or_conversation() {
    run_scenario("_evt007s5", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());

        // 账号 A 的父消息 "42" 位于群 g-1。
        let parent_a = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "42",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent A failed: {e}"))?
            .source_event_id()
            .clone();

        // 账号 B 的子引用 "42"（同群 ID 但不同账号）：绝不串联到 A 的父。
        let cross_account_child = store
            .insert_message_if_absent(&envelope(
                "acc-b",
                "c-b",
                ConversationKind::Group,
                "g-1",
                "carol",
                101,
                Some("42"),
            ))
            .await
            .map_err(|e| format!("insert cross-account child failed: {e}"))?
            .source_event_id()
            .clone();
        assert!(
            is_pending(&db, cross_account_child.as_str()).await,
            "跨账号同名父必须 fail-closed，保持 unresolved"
        );

        // 同账号、同群 ID 名但实际是不同群的子：绝不串联到 g-1 的父。
        let cross_group_child = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-g2",
                ConversationKind::Group,
                "g-2",
                "bob",
                102,
                Some("42"),
            ))
            .await
            .map_err(|e| format!("insert cross-group child failed: {e}"))?
            .source_event_id()
            .clone();
        assert!(
            is_pending(&db, cross_group_child.as_str()).await,
            "同账号跨群同名父必须 fail-closed，保持 unresolved"
        );

        // 同账号私聊子引用群内同名 "42"：群与私聊绝不串联。
        let private_child = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-priv",
                ConversationKind::Private,
                "p-1",
                "alice",
                103,
                Some("42"),
            ))
            .await
            .map_err(|e| format!("insert private child failed: {e}"))?
            .source_event_id()
            .clone();
        assert!(
            is_pending(&db, private_child.as_str()).await,
            "群与私聊同名父必须 fail-closed，保持 unresolved"
        );

        // 同账号同群的正主子事件必须解析成功（对照）。
        let same_scope_child = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-g1",
                ConversationKind::Group,
                "g-1",
                "bob",
                104,
                Some("42"),
            ))
            .await
            .map_err(|e| format!("insert same-scope child failed: {e}"))?
            .source_event_id()
            .clone();
        assert_eq!(
            resolved_parent_of(&db, same_scope_child.as_str())
                .await
                .as_deref(),
            Some(parent_a.as_str()),
            "同账号同会话子事件必须解析到真实父"
        );

        // 父后到回填同样不跨账号/跨会话：账号 B 的父 "42" 只修复 B 的同会话子。
        let parent_b = store
            .insert_message_if_absent(&envelope(
                "acc-b",
                "42",
                ConversationKind::Group,
                "g-1",
                "carol",
                105,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent B failed: {e}"))?
            .source_event_id()
            .clone();
        assert_eq!(
            resolved_parent_of(&db, cross_account_child.as_str())
                .await
                .as_deref(),
            Some(parent_b.as_str()),
            "账号 B 自己的父必须解析账号 B 的同会话子"
        );
        assert!(
            is_pending(&db, cross_group_child.as_str()).await,
            "群 g-2 的子不得被 g-1 的父回填"
        );
        assert!(is_pending(&db, private_child.as_str()).await);
        Ok(())
    })
    .await;
}

// ── 6. 事务失败零半提交，恢复后可重试 ───────────────────────────────────

#[tokio::test]
#[ignore]
async fn transaction_failure_rolls_back_cleanly_and_recovers() {
    run_scenario("_evt007s6", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());

        // 6a. 子事务失败：同一批含 poison 与正常消息，任何 SourceEvent/pending 不得落库。
        db.execute_unprepared(
            "ALTER TABLE secretary_source_events ADD CONSTRAINT chk_evt007_poison \
             CHECK (platform_event_id <> 'evt007-poison')",
        )
        .await
        .map_err(|e| format!("install poison check failed: {e}"))?;
        let batch = vec![
            envelope("acc-a", "evt007-poison", ConversationKind::Group, "g-1", "bob", 100, Some("p-6")),
            envelope("acc-a", "normal", ConversationKind::Group, "g-1", "bob", 101, Some("p-6")),
        ];
        let failed = store.insert_messages_if_absent(&batch).await;
        assert!(
            matches!(failed, Err(personal_secretary::InboundEventStoreError::Database(_))),
            "约束失败必须分类为 Database（暂态），不得伪装成 InvalidData"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events \
                 WHERE platform_event_id IN ('evt007-poison', 'normal')",
                Vec::new(),
            )
            .await,
            0,
            "子事务失败必须零半提交"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events \
                 WHERE reply_to_platform_event_id = 'p-6'",
                Vec::new(),
            )
            .await,
            0,
            "不得残留任何 pending"
        );
        db.execute_unprepared("ALTER TABLE secretary_source_events DROP CHECK chk_evt007_poison")
            .await
            .map_err(|e| format!("drop poison check failed: {e}"))?;

        // 6b. 父解析事务失败：父事件插入与 pending 回填同一事务，UPDATE 违反 CHECK
        //     时父 SourceEvent 也必须整体回滚；恢复后重试成功。
        let child_id = store
            .insert_message_if_absent(&envelope("acc-a", "c-6", ConversationKind::Group, "g-1", "bob", 200, Some("p-6")))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();
        assert!(is_pending(&db, child_id.as_str()).await);
        // reply_to_event_id 参与外键 fk_secretary_source_reply，MySQL 禁止 CHECK 引用
        // 外键列（3823），改用 BEFORE UPDATE 触发器精确拒绝"从 pending 解析"的 UPDATE。
        db.execute_unprepared(
            "CREATE TRIGGER trg_evt007_deny_reply BEFORE UPDATE ON secretary_source_events \
             FOR EACH ROW BEGIN \
               IF NEW.reply_to_event_id IS NOT NULL AND OLD.reply_to_event_id IS NULL THEN \
                 SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'evt007 forced rollback'; \
               END IF; \
             END",
        )
        .await
        .map_err(|e| format!("install deny-reply trigger failed: {e}"))?;

        let parent_failed = store
            .insert_message_if_absent(&envelope("acc-a", "p-6", ConversationKind::Group, "g-1", "alice", 199, None))
            .await;
        assert!(
            matches!(parent_failed, Err(personal_secretary::InboundEventStoreError::Database(_))),
            "父解析事务失败必须整体回滚"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events WHERE platform_event_id = 'p-6'",
                Vec::new(),
            )
            .await,
            0,
            "父 SourceEvent 不得残留（零半提交）"
        );
        assert!(
            is_pending(&db, child_id.as_str()).await,
            "子事件必须保持 unresolved，无部分正式关系"
        );
        db.execute_unprepared("DROP TRIGGER IF EXISTS trg_evt007_deny_reply")
            .await
            .map_err(|e| format!("drop deny-reply trigger failed: {e}"))?;

        let parent_id = store
            .insert_message_if_absent(&envelope("acc-a", "p-6", ConversationKind::Group, "g-1", "alice", 199, None))
            .await
            .map_err(|e| format!("recover parent insert failed: {e}"))?
            .source_event_id()
            .clone();
        assert_eq!(
            resolved_parent_of(&db, child_id.as_str()).await.as_deref(),
            Some(parent_id.as_str()),
            "恢复后重试必须完成解析"
        );
        assert!(!is_pending(&db, child_id.as_str()).await);
        Ok(())
    })
    .await;
}

// ── 7. 并发父子处理：最终恰好一条正式关系 ───────────────────────────────

#[tokio::test]
#[ignore]
async fn concurrent_parent_child_yields_exactly_one_relation() {
    run_scenario("_evt007s7", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());

        let child_store = Arc::clone(&store);
        let parent_store = Arc::clone(&store);
        let (child_result, parent_result) = tokio::join!(
            tokio::spawn(async move {
                child_store
                    .insert_message_if_absent(&envelope(
                        "acc-a",
                        "c-7",
                        ConversationKind::Group,
                        "g-1",
                        "bob",
                        101,
                        Some("p-7"),
                    ))
                    .await
            }),
            tokio::spawn(async move {
                parent_store
                    .insert_message_if_absent(&envelope(
                        "acc-a",
                        "p-7",
                        ConversationKind::Group,
                        "g-1",
                        "alice",
                        100,
                        None,
                    ))
                    .await
            }),
        );
        // tokio::join! 已等待两个任务：外层 JoinError，内层业务错误。
        let child = child_result
            .map_err(|e| format!("child task panicked: {e}"))?
            .map_err(|e| format!("concurrent child failed: {e}"))?;
        let parent = parent_result
            .map_err(|e| format!("parent task panicked: {e}"))?
            .map_err(|e| format!("concurrent parent failed: {e}"))?;
        let child_id = child.source_event_id().clone();
        let parent_id = parent.source_event_id().clone();

        // 无论交错顺序，最终恰好一条正式关系；重放两者仍不新增。
        let events_after = scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_source_events",
            Vec::new(),
        )
        .await;
        assert_eq!(events_after, 2, "并发插入不得产生重复 SourceEvent");
        let resolved = resolved_parent_of(&db, child_id.as_str()).await;
        assert_eq!(
            resolved.as_deref(),
            Some(parent_id.as_str()),
            "并发父/子处理后子事件必须恰好解析到真实父"
        );
        assert!(!is_pending(&db, child_id.as_str()).await);

        // 并发重放一轮（Duplicate 路径同样并发触发父回填与自愈）。
        let (c2, p2) = tokio::join!(
            tokio::spawn(insert_duplicate(
                Arc::clone(&store),
                envelope(
                    "acc-a",
                    "c-7",
                    ConversationKind::Group,
                    "g-1",
                    "bob",
                    101,
                    Some("p-7")
                )
            )),
            tokio::spawn(insert_duplicate(
                Arc::clone(&store),
                envelope(
                    "acc-a",
                    "p-7",
                    ConversationKind::Group,
                    "g-1",
                    "alice",
                    100,
                    None
                )
            )),
        );
        c2.map_err(|e| format!("child replay panicked: {e}"))?
            .map_err(|e| format!("child replay failed: {e}"))?;
        p2.map_err(|e| format!("parent replay panicked: {e}"))?
            .map_err(|e| format!("parent replay failed: {e}"))?;
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events",
                Vec::new(),
            )
            .await,
            2,
            "重放不得新增 SourceEvent"
        );
        assert_eq!(
            resolved_parent_of(&db, child_id.as_str()).await.as_deref(),
            Some(parent_id.as_str()),
            "并发重放后仍必须恰好一条正式关系"
        );
        Ok(())
    })
    .await;
}

async fn insert_duplicate(
    store: Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    message: InboundMessageEnvelope,
) -> Result<(), String> {
    let outcome = store
        .insert_message_if_absent(&message)
        .await
        .map_err(|e| e.to_string())?;
    assert!(
        matches!(
            outcome,
            personal_secretary::IngestMessageOutcome::Duplicate { .. }
        ),
        "重放必须返回 Duplicate"
    );
    Ok(())
}

// ── 8. 父永久缺失：保持 unresolved/uncertain，不静默删除 ────────────────

#[tokio::test]
#[ignore]
async fn parent_permanently_missing_stays_unresolved() {
    run_scenario("_evt007s8", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());

        let child_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-8",
                ConversationKind::Group,
                "g-1",
                "bob",
                101,
                Some("never-arrives"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();

        // 父事件从未到达：pending 保持可查询，绝不静默删除或宣称完整。
        assert!(
            is_pending(&db, child_id.as_str()).await,
            "父永久缺失时必须保持 unresolved"
        );
        assert!(
            resolved_parent_of(&db, child_id.as_str()).await.is_none(),
            "不得猜测或伪造父事件"
        );

        // 线程投影照常处理（子事件在自己的线程，不产生 Reply 边）。
        let projection = projection_use_case(db.clone());
        let run = projection
            .run_once()
            .await
            .map_err(|e| format!("project child failed: {e}"))?
            .expect("child must be projected");
        assert_eq!(run.events_projected, 1);
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_thread_relations",
                Vec::new(),
            )
            .await,
            0,
            "无父事件时不得创建 Reply 正式关系"
        );
        assert!(
            thread_of(&db, child_id.as_str()).await.is_some(),
            "无父事件不得影响事件本身的投影消费"
        );
        Ok(())
    })
    .await;
}

// ── 9. 投影已领取未提交：Reply 解析后旧计划必须 LeaseLost ───────────────

#[tokio::test]
#[ignore]
async fn claimed_projection_rejected_after_reply_resolution() {
    run_scenario("_evt007s9", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let projection_store = build_mysql_thread_projection_store(db.clone());
        let planner =
            DeterministicThreadPlanner::new(DeterministicThreadPolicy::new(3600).unwrap());

        // 子先到（pending）。
        let child_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-9",
                ConversationKind::Group,
                "g-1",
                "bob",
                101,
                Some("p-9"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();

        // 投影 Worker 已领取子事件并生成旧计划（lease 有效），但尚未提交。
        let claimed = projection_store
            .claim_projection_batch(100, 60, 3600)
            .await
            .map_err(|e| format!("claim failed: {e}"))?
            .expect("child must be claimed");
        assert_eq!(claimed.events.len(), 1, "旧计划恰好包含子事件");

        // 父事件在此窗口到达：解析 + 撤销投影租约。
        let parent_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-9",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e}"))?
            .source_event_id()
            .clone();
        assert_eq!(
            resolved_parent_of(&db, child_id.as_str()).await.as_deref(),
            Some(parent_id.as_str())
        );
        assert!(
            !projection_claim_exists(&db, child_id.as_str()).await,
            "解析必须撤销子事件的投影租约"
        );

        // 旧计划提交必须因租约检查失败而拒绝，不能把子事件写回旧线程。
        let old_plan = planner
            .plan(claimed)
            .map_err(|e| format!("plan old batch failed: {e}"))?;
        let commit_error = projection_store.commit_projection(&old_plan).await;
        assert!(
            matches!(
                commit_error,
                Err(personal_secretary::InboundEventStoreError::LeaseLost)
            ),
            "旧计划提交必须返回 LeaseLost，实际: {commit_error:?}"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_event_threads",
                Vec::new(),
            )
            .await,
            0,
            "被拒绝的旧计划不得创建任何线程"
        );

        // 重新投影：父子最终同线程，且存在真实 Reply 边。
        let projection = projection_use_case(db.clone());
        projection
            .run_once()
            .await
            .map_err(|e| format!("project parent failed: {e}"))?;
        projection
            .run_once()
            .await
            .map_err(|e| format!("reproject child failed: {e}"))?;
        assert_eq!(
            thread_of(&db, child_id.as_str()).await,
            thread_of(&db, parent_id.as_str()).await,
            "子事件最终必须进入父事件线程"
        );
        assert_eq!(
            reply_relation_count(&db, child_id.as_str(), parent_id.as_str()).await,
            1,
            "最终恰好一条 Reply 正式关系"
        );
        Ok(())
    })
    .await;
}

// ── 10. 提交后自愈失败：不破坏"已提交"契约，父重放可恢复 ────────────────

#[tokio::test]
#[ignore]
async fn self_heal_failure_keeps_committed_contract() {
    run_scenario("_evt007s10", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());

        // 父先到达。
        let parent_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-10",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e}"))?
            .source_event_id()
            .clone();

        // 遗留 pending 子（模拟早期缺陷/交错残留）：引用 p-10 但未解析。
        let acc_a_id = scalar_u64(
            &db,
            "SELECT id AS value FROM secretary_accounts WHERE platform_account_id = 'acc-a'",
            Vec::new(),
        )
        .await;
        let g1_id = scalar_u64(
            &db,
            "SELECT c.id AS value FROM secretary_conversations c \
             JOIN secretary_accounts a ON a.id = c.account_id \
             WHERE a.platform_account_id = 'acc-a' AND c.platform_conversation_id = 'g-1'",
            Vec::new(),
        )
        .await;
        let legacy_id = uuid::Uuid::new_v4().to_string();
        db.execute_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "INSERT INTO secretary_source_events \
             (source_event_id, account_id, conversation_id, source_channel, platform_event_id, \
              event_type, actor_platform_id, actor_kind, message_role, occurred_at_unix_secs, \
              reply_to_platform_event_id, reply_to_event_id, processing_status, received_at, created_at) \
             VALUES (?, ?, ?, 'napcat', 'y-10', 'message', 'bob', 'external', \
                     'external_observation', 101, 'p-10', NULL, 'pending', NOW(6), NOW(6))",
            vec![legacy_id.clone().into(), acc_a_id.into(), g1_id.into()],
        ))
        .await
        .map_err(|e| format!("seed legacy pending child failed: {e}"))?;
        // 候选队列（Codex 第四轮复核 #5）：遗留 pending 子必须同步入队，
        // reconcile 从此表出发不再扫描全部 source_events。
        db.execute_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "INSERT INTO secretary_reply_reconcile_claims (source_event_id) VALUES (?)",
            vec![legacy_id.into()],
        ))
        .await
        .map_err(|e| format!("seed legacy candidate claim failed: {e}"))?;

        // 拒绝一切 pending→resolved 的 UPDATE：主事务内回填不受影响（本批无 pending
        // 子），只有提交后自愈会命中并失败。
        db.execute_unprepared(
            "CREATE TRIGGER trg_evt007_deny_self_heal BEFORE UPDATE ON secretary_source_events \
             FOR EACH ROW BEGIN \
               IF NEW.reply_to_event_id IS NOT NULL AND OLD.reply_to_event_id IS NULL THEN \
                 SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'evt007 self-heal forced failure'; \
               END IF; \
             END",
        )
        .await
        .map_err(|e| format!("install self-heal trigger failed: {e}"))?;

        // 消息 X 带 Reply 段引用 p-10：主事务成功（X 直接关联父），提交后自愈
        // 尝试修复遗留子失败——但 insert 必须返回 Ok（已提交契约不破坏）。
        let x = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "x-10",
                ConversationKind::Group,
                "g-1",
                "carol",
                102,
                Some("p-10"),
            ))
            .await;
        assert!(
            matches!(
                &x,
                Ok(personal_secretary::IngestMessageOutcome::Accepted { .. })
            ),
            "主批事务已提交，自愈失败不得使 insert 返回 Err，实际: {x:?}"
        );
        let x_outcome = x.expect("X 主批事务必须成功（已在上方断言 Accepted）");
        let x_id = x_outcome.source_event_id().clone();
        assert_eq!(
            resolved_parent_of(&db, x_id.as_str()).await.as_deref(),
            Some(parent_id.as_str()),
            "X 自身必须在主事务内完成解析"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events \
                 WHERE platform_event_id = 'y-10' AND reply_to_event_id IS NULL",
                Vec::new(),
            )
            .await,
            1,
            "自愈失败后遗留子保持 unresolved（无半提交），等待父事件重放"
        );

        // 故障恢复后，后台修复 Worker 路径（Codex 复核 P1-1）完成修复：
        // 不重放任何消息，只领取 unresolved 候选并重试解析。
        db.execute_unprepared("DROP TRIGGER IF EXISTS trg_evt007_deny_self_heal")
            .await
            .map_err(|e| format!("drop self-heal trigger failed: {e}"))?;
        let reconcile_store = build_mysql_reply_reconcile_store(db.clone());
        let reconcile = personal_secretary::ReconcilePendingRepliesUseCase::new(
            reconcile_store,
            personal_secretary::ReconcileBudget::new(10, 60, 1, 2),
        );
        let outcome = reconcile
            .run_one()
            .await
            .map_err(|e| format!("reconcile run failed: {e}"))?;
        assert!(
            outcome.claimed >= 1,
            "unresolved 遗留子必须被修复 Worker 领取"
        );
        assert!(
            outcome.resolved >= 1,
            "修复轮次必须解析至少一个候选，实际: {outcome:?}"
        );
        let y_id = scalar_string(
            &db,
            "SELECT source_event_id AS value FROM secretary_source_events \
             WHERE platform_event_id = 'y-10'",
            Vec::new(),
        )
        .await;
        assert_eq!(
            resolved_parent_of(&db, &y_id).await.as_deref(),
            Some(parent_id.as_str()),
            "故障解除后无需再次插入消息，后台修复必须完成解析"
        );
        assert!(!is_pending(&db, &y_id).await);
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_reply_reconcile_claims",
                Vec::new(),
            )
            .await,
            0,
            "已解析候选的退避簿行必须清理"
        );
        Ok(())
    })
    .await;
}

// ── 11. 非 Reply 证据边在投影失效中保留 ─────────────────────────────────

#[tokio::test]
#[ignore]
async fn non_reply_relations_preserved_on_projection_invalidation() {
    run_scenario("_evt007s11", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let projection = projection_use_case(db.clone());

        // 子（pending）与独立事件 D 同会话入库。
        let child_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-11",
                ConversationKind::Group,
                "g-1",
                "bob",
                101,
                Some("p-11"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();
        let d_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "d-11",
                ConversationKind::Group,
                "g-1",
                "dave",
                102,
                None,
            ))
            .await
            .map_err(|e| format!("insert D failed: {e}"))?
            .source_event_id()
            .clone();

        // 投影：子建线程 T1，D 因同会话窗口加入 T1。
        projection
            .run_once()
            .await
            .map_err(|e| format!("project failed: {e}"))?;
        let t1 = thread_of(&db, child_id.as_str())
            .await
            .expect("child must be projected into T1");
        assert_eq!(
            thread_of(&db, d_id.as_str()).await.as_deref(),
            Some(t1.as_str()),
            "D 必须与子同线程（前置）"
        );

        // 手工建立非确定性证据边（explicit_project_id，不属于本次 Reply 修复）。
        db.execute_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "INSERT INTO secretary_thread_relations \
             (relation_id, thread_id, from_event_id, to_event_id, relation_kind, \
              confidence_bps, reason, created_at) \
             VALUES (UUID(), ?, ?, ?, 'explicit_project_id', 10000, \
                     'test evidence not part of reply resolution', UTC_TIMESTAMP(6))",
            vec![
                t1.clone().into(),
                child_id.as_str().into(),
                d_id.as_str().into(),
            ],
        ))
        .await
        .map_err(|e| format!("seed explicit relation failed: {e}"))?;

        // 父到达解析：确定性边失效不得删除非 Reply 证据边。
        let parent_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-11",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e:?}"))?
            .source_event_id()
            .clone();
        assert_eq!(
            resolved_parent_of(&db, child_id.as_str()).await.as_deref(),
            Some(parent_id.as_str())
        );
        // 事件迁入父线程后，原关系的 from_event_id 已离开旧线程，
        // 而 to_event_id 仍留在旧线程——两端不在同一线程。当前关系模型
        // 无 historical/active 语义，跨线程关系不可保留（Codex 第三轮复核 P1-4）。
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_thread_relations \
                 WHERE from_event_id = ?",
                vec![child_id.as_str().into()],
            )
            .await,
            0,
            "子事件所有出边在解析时全部失效（含 explicit_project_id）"
        );

        // T1 仍有成员 D，不得被关闭；子重新投影进父线程。
        assert_eq!(
            thread_status(&db, &t1).await.as_deref(),
            Some("open"),
            "仍有成员的旧线程不得关闭"
        );
        projection
            .run_once()
            .await
            .map_err(|e| format!("project parent failed: {e}"))?;
        projection
            .run_once()
            .await
            .map_err(|e| format!("reproject child failed: {e}"))?;
        assert_eq!(
            thread_of(&db, child_id.as_str()).await,
            thread_of(&db, parent_id.as_str()).await,
            "子事件必须进入父事件线程"
        );
        assert_ne!(
            thread_of(&db, child_id.as_str()).await.as_deref(),
            Some(t1.as_str()),
            "子事件必须离开旧线程"
        );
        assert_eq!(
            thread_of(&db, d_id.as_str()).await.as_deref(),
            Some(t1.as_str()),
            "D 必须留在旧线程"
        );
        Ok(())
    })
    .await;
}

// ── 12. 真实 BackfillGapUseCase 路径：父经回补到达完成解析 ──────────────

/// 确定性历史来源：预置一页父消息（BackfillGapUseCase 的真实来源端口实现）。
struct FakeBackfillSource {
    page: personal_secretary::BackfillPage,
}

#[async_trait::async_trait]
impl personal_secretary::HistoryBackfillSourceT for FakeBackfillSource {
    async fn fetch_page(
        &self,
        _scope: &personal_secretary::BackfillScope,
        _cursor: Option<&personal_secretary::BackfillCursor>,
        _direction: personal_secretary::BackfillReadDirection,
        _page_size: u32,
    ) -> Result<personal_secretary::BackfillPage, personal_secretary::BackfillSourceError> {
        Ok(self.page.clone())
    }

    fn history_start_evidence_proven(&self) -> bool {
        true
    }

    fn page_order_evidence_proven(&self) -> bool {
        true
    }

    fn account_conversation_set_proven(&self) -> bool {
        false // 真实 NapCat 无法证明账号会话集合完整
    }
}

#[tokio::test]
#[ignore]
async fn backfill_use_case_resolves_delayed_reply() {
    run_scenario("_evt007s12", |db| async move {
        use personal_secretary::{
            BackfillAnchor, BackfillBudget, BackfillGapUseCase, BackfillHistoryItem,
            IngestionGapReason,
        };
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());

        // 连接周期 → 实时子消息（带 epoch 写连续性）→ uncertain Gap。
        let account = personal_secretary::SourceAccountRef::new(
            personal_secretary::MessageSource::NapCat,
            "acc-a",
        )
        .unwrap();
        let epoch = store
            .begin_connection(&account)
            .await
            .map_err(|e| format!("begin connection failed: {e}"))?;
        store
            .mark_connection_connected(&epoch)
            .await
            .map_err(|e| format!("mark connected failed: {e}"))?;
        let child_id = store
            .insert_message_if_absent(
                &envelope(
                    "acc-a",
                    "c-12",
                    ConversationKind::Group,
                    "g-1",
                    "bob",
                    101,
                    Some("p-12"),
                )
                .observed_in(epoch.clone()),
            )
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();
        assert!(is_pending(&db, child_id.as_str()).await);
        store
            .mark_connection_uncertain(&epoch, IngestionGapReason::QueueOverflow)
            .await
            .map_err(|e| format!("mark uncertain failed: {e}"))?;
        // 关闭空窗：仅当 Gap 的 gap_ended_at 非空（下一次重连）时才可被回补领取。
        let epoch2 = store
            .begin_connection(&account)
            .await
            .map_err(|e| format!("begin epoch2 failed: {e}"))?;
        store
            .mark_connection_connected(&epoch2)
            .await
            .map_err(|e| format!("mark epoch2 connected failed: {e}"))?;

        // 真实 Backfill 用例：来源端口预置父消息页，经统一幂等入口入库。
        let parent_item = BackfillHistoryItem {
            envelope: envelope(
                "acc-a",
                "p-12",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ),
            anchor: BackfillAnchor::new("p-12", "seq-12"),
        };
        let source = Arc::new(FakeBackfillSource {
            page: personal_secretary::BackfillPage {
                items: vec![parent_item],
                continuation: personal_secretary::BackfillContinuation::ProvenHistoryStart,
            },
        });
        let budget = BackfillBudget {
            page_size: 100,
            max_pages_per_scope: 20,
            max_events_per_run: 2000,
            max_concurrency: 2,
            lease_secs: 60,
            retry_initial_ms: 1,
            retry_max_ms: 2,
        };
        // 与生产装配一致：回补用例使用独立的组合仓储实例，与实时连续性仓储共享同一 schema。
        let backfill_store = build_mysql_backfill_store(db.clone(), 60);
        let source_port: Arc<dyn personal_secretary::HistoryBackfillSourceT> = source;
        let use_case = BackfillGapUseCase::new(backfill_store, source_port, budget);
        let outcome = use_case
            .run_one()
            .await
            .map_err(|e| format!("backfill run failed: {e}"))?
            .expect("a claimable gap must be processed");

        assert!(
            outcome.evidence.scopes[0].accepted >= 1,
            "父消息必须经回补统一入口 Accepted 入库"
        );
        assert!(
            resolved_parent_of(&db, child_id.as_str()).await.is_some(),
            "父经 Backfill 到达后子事件必须完成解析"
        );
        assert!(!is_pending(&db, child_id.as_str()).await);
        Ok(())
    })
    .await;
}

// ── 13. 后台修复：退避、有界领取与跨路径清理 ────────────────────────────

#[tokio::test]
#[ignore]
async fn reconcile_backs_off_bounded_and_cleans_claims() {
    run_scenario("_evt007s13", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let reconcile_store = build_mysql_reply_reconcile_store(db.clone());

        // 三个 unresolved 子事件（父均缺失）。
        for id in ["c-13a", "c-13b", "c-13c"] {
            store
                .insert_message_if_absent(&envelope(
                    "acc-a",
                    id,
                    ConversationKind::Group,
                    "g-1",
                    "bob",
                    101,
                    Some("p-13"),
                ))
                .await
                .map_err(|e| format!("insert {id} failed: {e}"))?;
        }
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events \
                 WHERE reply_to_platform_event_id = 'p-13' AND reply_to_event_id IS NULL",
                Vec::new(),
            )
            .await,
            3,
            "前置：3 个 unresolved 候选"
        );

        // 轮 1：batch_size=2 有界领取，父不在 → 全部退避（退避 60s，保证轮 2
        // 在退避期内运行，确定性验证热循环被阻止）。
        let reconcile = personal_secretary::ReconcilePendingRepliesUseCase::new(
            reconcile_store,
            personal_secretary::ReconcileBudget::new(2, 60, 60_000, 120_000),
        );
        let outcome = reconcile
            .run_one()
            .await
            .map_err(|e| format!("reconcile round1 failed: {e}"))?;
        assert_eq!(outcome.claimed, 2, "有界领取不得超过 batch_size");
        assert_eq!(outcome.still_pending, 2, "父缺失候选必须退避");
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_reply_reconcile_claims",
                Vec::new(),
            )
            .await,
            3,
            "候选队列包含全部 3 个 unresolved 子事件（含未领取的第三个）"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_reply_reconcile_claims \
                 WHERE lease_token IS NOT NULL AND lease_expires_at IS NOT NULL",
                Vec::new(),
            )
            .await,
            0,
            "处理后退避簿必须释放租约（fencing 令牌已清除）"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_reply_reconcile_claims \
                 WHERE lease_token IS NULL AND next_eligible_at IS NOT NULL",
                Vec::new(),
            )
            .await,
            2,
            "退避后租约必须释放且写入 next_eligible_at"
        );

        // 轮 2（退避未到期）：只能领取从未领取过的第三个候选（有界领取逐轮
        // 消费），已退避的两个不得被重复领取——热循环被阻止。
        let outcome2 = reconcile
            .run_one()
            .await
            .map_err(|e| format!("reconcile round2 failed: {e}"))?;
        assert_eq!(outcome2.claimed, 1, "退避期内只能领取未领取过的候选");
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_reply_reconcile_claims \
                 WHERE attempts >= 2",
                Vec::new(),
            )
            .await,
            0,
            "退避中的候选不得被再次领取（attempts 不得重复递增）"
        );

        // 父经实时路径到达：解析全部 pending（含未领取的 c-13c），
        // 退避簿行必须随主路径解析一并清理。
        let parent_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-13",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e}"))?
            .source_event_id()
            .clone();
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events \
                 WHERE reply_to_platform_event_id = 'p-13' AND reply_to_event_id IS NULL",
                Vec::new(),
            )
            .await,
            0,
            "父到达后全部 pending 解析"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_reply_reconcile_claims",
                Vec::new(),
            )
            .await,
            0,
            "主路径解析必须清理退避簿行（不残留已解析候选）"
        );
        // 投影后：退避候选与父事件最终同线程并存在正式 Reply 边。
        let projection = projection_use_case(db.clone());
        projection
            .run_once()
            .await
            .map_err(|e| format!("project parent failed: {e}"))?;
        projection
            .run_once()
            .await
            .map_err(|e| format!("reproject children failed: {e}"))?;
        let child_b_id = scalar_string(
            &db,
            "SELECT source_event_id AS value FROM secretary_source_events \
             WHERE platform_event_id = 'c-13b'",
            Vec::new(),
        )
        .await;
        assert_eq!(
            thread_of(&db, &child_b_id).await,
            thread_of(&db, parent_id.as_str()).await,
            "退避候选必须与父事件同线程"
        );
        assert_eq!(
            reply_relation_count(&db, &child_b_id, parent_id.as_str()).await,
            1,
            "退避候选最终仍解析为正式 Reply 关系"
        );
        Ok(())
    })
    .await;
}

// ── 14. 线程已终态：close 不写虚假历史，终态线程拒绝新成员 ──────────────

#[tokio::test]
#[ignore]
async fn closed_thread_rejects_members_and_writes_no_fake_history() {
    run_scenario("_evt007s14", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let projection = projection_use_case(db.clone());

        // 子 pending → 投影进 T1。
        let child_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-14",
                ConversationKind::Group,
                "g-1",
                "bob",
                101,
                Some("p-14"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();
        projection
            .run_once()
            .await
            .map_err(|e| format!("project child failed: {e}"))?;
        let t1 = thread_of(&db, child_id.as_str())
            .await
            .expect("child must be in T1");

        // 语义事务在 close 之前把线程迁移到终态（resolved），同时提交语义派生。
        db.execute_unprepared(&format!(
            "INSERT INTO secretary_thread_claims \
             (claim_id, thread_id, claim_kind, claimant_channel, claimant_account, \
              claimant_actor_id, statement, status, confidence_bps, created_at, updated_at) \
             VALUES (UUID(), '{t1}', 'request', 'napcat', 'acc-a', 'bob', \
                     'stale derivation on resolved thread', 'proposed', 5000, \
                     UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))"
        ))
        .await
        .map_err(|e| format!("seed claim failed: {e}"))?;
        db.execute_unprepared(&format!(
            "UPDATE secretary_event_threads SET status = 'resolved', updated_at = UTC_TIMESTAMP(6) \
             WHERE thread_id = '{t1}'"
        ))
        .await
        .map_err(|e| format!("mark thread resolved failed: {e}"))?;

        // 父到达解析：终态线程不写关闭历史，但必须在确认空线程后撤销已提交派生。
        let parent_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-14",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e}"))?
            .source_event_id()
            .clone();
        assert_eq!(
            resolved_parent_of(&db, child_id.as_str()).await.as_deref(),
            Some(parent_id.as_str())
        );
        assert_eq!(
            thread_status(&db, &t1).await.as_deref(),
            Some("resolved"),
            "线程保持语义事务写入的终态"
        );
        assert!(
            !has_system_recovery_history(&db, &t1).await,
            "终态线程不得写入虚假的 system_recovery 关闭历史"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_thread_status_history WHERE thread_id = ?",
                vec![t1.as_str().into()],
            )
            .await,
            0,
            "终态线程不得产生任何状态历史"
        );
        // 终态空线程的已提交语义派生仍必须撤销（Codex 第三轮复核 P1-2）。
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_thread_claims \
                 WHERE thread_id = ? AND status = 'withdrawn'",
                vec![t1.as_str().into()],
            )
            .await,
            1,
            "终态空线程上的 claim 必须标记 withdrawn"
        );

        // 重投影：终态线程拒绝新成员（投影侧 FOR UPDATE 复验），
        // 子事件进入父线程，不会插回 T1。
        projection
            .run_once()
            .await
            .map_err(|e| format!("project parent failed: {e}"))?;
        projection
            .run_once()
            .await
            .map_err(|e| format!("reproject child failed: {e}"))?;
        assert_eq!(
            thread_of(&db, child_id.as_str()).await,
            thread_of(&db, parent_id.as_str()).await,
            "子事件必须进入父线程"
        );
        assert_ne!(
            thread_of(&db, child_id.as_str()).await.as_deref(),
            Some(t1.as_str()),
            "终态线程不得接纳新成员"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_thread_events WHERE thread_id = ?",
                vec![t1.as_str().into()],
            )
            .await,
            0,
            "终态线程保持无成员（拒绝 closed 后再插入）"
        );
        Ok(())
    })
    .await;
}

// ── 15. 语义派生随空线程关闭一并撤销 ────────────────────────────────────

#[tokio::test]
#[ignore]
async fn semantic_derivations_revoked_with_closed_thread() {
    run_scenario("_evt007s15", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let projection = projection_use_case(db.clone());

        // 子 pending → 投影进 T1。
        let child_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-15",
                ConversationKind::Group,
                "g-1",
                "bob",
                101,
                Some("p-15"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();
        projection
            .run_once()
            .await
            .map_err(|e| format!("project child failed: {e}"))?;
        let t1 = thread_of(&db, child_id.as_str())
            .await
            .expect("child must be in T1");

        // 模拟语义 Worker 已提交的派生：claim、decision、open question、
        // 回复期待与批处理状态（含活跃租约）。
        db.execute_unprepared(&format!(
            "INSERT INTO secretary_thread_claims \
             (claim_id, thread_id, claim_kind, claimant_channel, claimant_account, \
              claimant_actor_id, statement, status, confidence_bps, created_at, updated_at) \
             VALUES (UUID(), '{t1}', 'request', 'napcat', 'acc-a', 'bob', \
                     'derived by semantic worker', 'proposed', 5000, \
                     UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))"
        ))
        .await
        .map_err(|e| format!("seed claim failed: {e}"))?;
        db.execute_unprepared(&format!(
            "INSERT INTO secretary_thread_decisions \
             (decision_id, thread_id, statement, status, confidence_bps, created_at, updated_at) \
             VALUES (UUID(), '{t1}', 'derived by semantic worker', 'proposed', 5000, \
                     UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))"
        ))
        .await
        .map_err(|e| format!("seed decision failed: {e}"))?;
        db.execute_unprepared(&format!(
            "INSERT INTO secretary_thread_open_questions \
             (question_id, thread_id, raised_by_channel, raised_by_account, raised_by_actor_id, \
              question, status, confidence_bps, created_at, updated_at) \
             VALUES (UUID(), '{t1}', 'napcat', 'acc-a', 'bob', \
                     'derived by semantic worker', 'open', 5000, \
                     UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))"
        ))
        .await
        .map_err(|e| format!("seed question failed: {e}"))?;
        db.execute_unprepared(&format!(
            "INSERT INTO secretary_response_expectations \
             (expectation_id, account_id, source_question_id, thread_id, due_at_unix_secs) \
             SELECT UUID(), t.account_id, q.question_id, t.thread_id, 2000000000 \
             FROM secretary_event_threads t \
             JOIN secretary_thread_open_questions q ON q.thread_id = t.thread_id \
             WHERE t.thread_id = '{t1}' LIMIT 1"
        ))
        .await
        .map_err(|e| format!("seed expectation failed: {e}"))?;
        db.execute_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "INSERT INTO secretary_thread_semantic_state \
             (thread_id, last_source_event_id, lease_token, lease_expires_at, attempts, updated_at) \
             VALUES (?, ?, 'lease-before-resolution', UTC_TIMESTAMP(6) + INTERVAL 60 SECOND, 1, UTC_TIMESTAMP(6))",
            vec![t1.clone().into(), child_id.as_str().into()],
        ))
        .await
        .map_err(|e| format!("seed semantic state failed: {e}"))?;

        // 父到达解析：子迁走 → T1 空 → 关闭并撤销全部派生。
        let parent_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-15",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e}"))?
            .source_event_id()
            .clone();
        assert_eq!(
            resolved_parent_of(&db, child_id.as_str()).await.as_deref(),
            Some(parent_id.as_str())
        );
        assert_eq!(
            thread_status(&db, &t1).await.as_deref(),
            Some("closed"),
            "空线程必须关闭"
        );
        assert!(
            has_system_recovery_history(&db, &t1).await,
            "必须保留 system_recovery 关闭审计"
        );
        // claims -> withdrawn，decisions -> revoked，questions -> dismissed，
        // expectations -> dismissed，语义批处理状态清除（含租约 fencing）。
        for (table, status) in [
            ("secretary_thread_claims", "withdrawn"),
            ("secretary_thread_decisions", "revoked"),
            ("secretary_thread_open_questions", "dismissed"),
        ] {
            assert_eq!(
                scalar_u64(
                    &db,
                    &format!(
                        "SELECT COUNT(*) AS value FROM {table} \
                         WHERE thread_id = ? AND status = '{status}'"
                    ),
                    vec![t1.as_str().into()],
                )
                .await,
                1,
                "{table} 必须迁移到失效终态 {status}"
            );
        }
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_response_expectations \
                 WHERE thread_id = ? AND expectation_status = 'dismissed'",
                vec![t1.as_str().into()],
            )
            .await,
            1,
            "回复期待必须随问题失效"
        );
        assert_eq!(
            semantic_state_count(&db, &t1).await,
            0,
            "语义批处理状态与租约必须清除"
        );
        assert!(
            !projection_claim_exists(&db, child_id.as_str()).await,
            "投影租约必须撤销"
        );
        Ok(())
    })
    .await;
}

// ── 16. 终态父线程拒绝自动接纳 Reply 子事件（Codex 第四轮复核 #4）────────
// 子事件回复已终态（resolved/closed）线程中的父事件时，planner 不得将子事件
// 推入终态线程，应创建新线程且不写跨线程 Reply 关系。

#[tokio::test]
#[ignore]
async fn reply_to_terminal_parent_thread_creates_new_thread() {
    run_scenario("_evt007s16", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let projection = projection_use_case(db.clone());

        // 父事件 → 投影进 T1。
        let parent_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-16",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e}"))?
            .source_event_id()
            .clone();
        projection
            .run_once()
            .await
            .map_err(|e| format!("project parent failed: {e}"))?;
        let t1 = thread_of(&db, parent_id.as_str())
            .await
            .expect("parent must be in T1");

        // 标记 T1 为终态（模拟语义线程 close）。
        db.execute_unprepared(&format!(
            "UPDATE secretary_event_threads SET status = 'closed', updated_at = UTC_TIMESTAMP(6) \
             WHERE thread_id = '{t1}'"
        ))
        .await
        .map_err(|e| format!("mark thread closed failed: {e}"))?;

        // 子事件（Reply 到父事件）→ pending，然后投影。
        let child_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-16",
                ConversationKind::Group,
                "g-1",
                "bob",
                200,
                Some("p-16"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();

        // 父事件到达后实时解析完成 Reply 映射。
        assert_eq!(
            resolved_parent_of(&db, child_id.as_str()).await.as_deref(),
            Some(parent_id.as_str()),
            "父到达后必须实时解析 Reply 映射"
        );

        // 投影 child：planner 应检测到父线程 T1 为终态，不将 child 放入 T1。
        projection
            .run_once()
            .await
            .map_err(|e| format!("project child failed: {e}"))?;

        let child_thread = thread_of(&db, child_id.as_str())
            .await
            .expect("child must be projected");
        assert_ne!(child_thread, t1, "子事件不得进入终态父线程");
        assert_ne!(
            thread_of(&db, parent_id.as_str()).await.as_deref(),
            Some(child_thread.as_str()),
            "父事件仍在原始线程中"
        );

        // 不得存在跨线程 Reply 关系（子在新线程，父在终态线程，不应连边）。
        assert_eq!(
            reply_relation_count(&db, child_id.as_str(), parent_id.as_str()).await,
            0,
            "终态父线程不得产生跨线程 Reply 关系"
        );
        Ok(())
    })
    .await;
}

// ── 17. 语义租约 fencing：终端分支删除 semantic_state 再撤销派生（#2）────
// 在终态空线程清理时，必须先删除 secretary_thread_semantic_state，
// 再撤销语义派生（claims/decisions/questions），避免并发语义 Worker
// 读取到已失效线程的过期语义批处理状态。

#[tokio::test]
#[ignore]
async fn terminal_thread_deletes_semantic_state_before_derive_revoke() {
    run_scenario("_evt007s17", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let projection = projection_use_case(db.clone());
        let semantic_store = build_mysql_thread_semantic_store(db.clone());

        // 子 pending → 投影进 T1。
        let child_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-17",
                ConversationKind::Group,
                "g-1",
                "bob",
                101,
                Some("p-17"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();
        projection
            .run_once()
            .await
            .map_err(|e| format!("project child failed: {e}"))?;
        let t1 = thread_of(&db, child_id.as_str())
            .await
            .expect("child must be in T1");

        // 真实语义 Worker 先领取批次但尚未提交。Reply 解析必须删除该租约，
        // 使下面构造的旧补丁在提交时得到 LeaseLost。
        let semantic_batch = semantic_store
            .claim_semantic_batch(100, 100_000, 60)
            .await
            .map_err(|e| format!("claim semantic batch failed: {e}"))?
            .expect("projected child must produce a semantic batch");
        assert_eq!(semantic_batch.thread_id.as_str(), t1);
        let semantic_event = semantic_batch
            .events
            .first()
            .expect("semantic batch must contain child event");
        let stale_claim_id = ThreadClaimId::new("stale-semantic-claim-17").unwrap();
        let stale_patch = ThreadSemanticPatch {
            claims: vec![ThreadClaimCandidate {
                claim_id: stale_claim_id.clone(),
                thread_id: semantic_batch.thread_id.clone(),
                kind: ClaimKind::Request,
                claimant: semantic_event.actor.clone(),
                statement: "stale semantic patch must not commit".into(),
                confidence_bps: 5_000,
                source_event_ids: vec![semantic_event.source_event_id.clone()],
            }],
            ..ThreadSemanticPatch::default()
        };

        // 另放入一条已提交派生，验证终态清理会保留审计行并撤销其状态。
        db.execute_unprepared(&format!(
            "INSERT INTO secretary_thread_claims \
             (claim_id, thread_id, claim_kind, claimant_channel, claimant_account, \
              claimant_actor_id, statement, status, confidence_bps, created_at, updated_at) \
             VALUES (UUID(), '{t1}', 'request', 'napcat', 'acc-a', 'bob', \
                     'derived by semantic worker', 'proposed', 5000, \
                     UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))"
        ))
        .await
        .map_err(|e| format!("seed claim failed: {e}"))?;

        // 关键（Codex 第四轮复核 P1-2）：先把 T1 标记为终端（resolved），
        // 模拟真实语义线程已完成的 close。父到达后 close_empty_thread_in_txn
        // 必须走 is_terminal 分支——DELETE semantic_state 再 revoke 派生。
        db.execute_unprepared(&format!(
            "UPDATE secretary_event_threads SET status = 'resolved', \
             updated_at = UTC_TIMESTAMP(6) WHERE thread_id = '{t1}'"
        ))
        .await
        .map_err(|e| format!("mark thread resolved failed: {e}"))?;

        // 父到达 → 解析 child → T1 变空 → 终端清理（is_terminal=true 分支）。
        store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-17",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e}"))?;

        // 验证终态线程上 semantic_state 已被删除（DELETE 先于 revoke）。
        assert_eq!(
            semantic_state_count(&db, &t1).await,
            0,
            "终态空线程的 semantic_state 必须被删除"
        );

        let stale_commit = semantic_store
            .commit_semantic_patch(&semantic_batch, &stale_patch)
            .await;
        assert!(
            matches!(
                stale_commit,
                Err(personal_secretary::InboundEventStoreError::LeaseLost)
            ),
            "Reply 解析撤销语义租约后，旧补丁提交必须返回 LeaseLost，实际: {stale_commit:?}"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_thread_claims WHERE claim_id = ?",
                vec![stale_claim_id.as_str().into()],
            )
            .await,
            0,
            "LeaseLost 的旧语义补丁不得产生派生行"
        );

        // 验证 claim 已撤销（证明 DELETE 在 revoke 之前且 revoke 正常执行）。
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_thread_claims \
                 WHERE thread_id = ? AND status = 'withdrawn'",
                vec![t1.as_str().into()],
            )
            .await,
            1,
            "终态空线程上的 claim 必须标记 withdrawn"
        );
        Ok(())
    })
    .await;
}

// ── 18. Relation 清理覆盖入边方向（Codex 第四轮复核 #3）──────────────────
// Reply 解析使子事件迁出线程时，必须删除以子事件为 from 或 to 的所有关系边：
// `r.from_event_id = child.source_event_id OR r.to_event_id = child.source_event_id`。

#[tokio::test]
#[ignore]
async fn relation_cleanup_deletes_both_edge_directions() {
    run_scenario("_evt007s18", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let projection = projection_use_case(db.clone());

        // 插入 grandparent（稍后用于构造入边）。
        let gp_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "gp-18",
                ConversationKind::Group,
                "g-1",
                "alice",
                90,
                None,
            ))
            .await
            .map_err(|e| format!("insert grandparent failed: {e}"))?
            .source_event_id()
            .clone();

        // 子先到（父未到）→ pending → 投影进原线程。
        let child_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-18",
                ConversationKind::Group,
                "g-1",
                "bob",
                200,
                Some("p-18"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?
            .source_event_id()
            .clone();

        // 投影 grandparent 和子（两轮）。
        for _ in 0..2 {
            projection
                .run_once()
                .await
                .map_err(|e| format!("project round failed: {e}"))?;
        }
        let child_t = thread_of(&db, child_id.as_str())
            .await
            .expect("child projected");

        // 手动插入入边（gp → child），模拟非确定性/语义边。
        let inbound_edge_id = uuid::Uuid::new_v4().to_string();
        let child_id_str = child_id.as_str().to_owned();
        let gp_id_str = gp_id.as_str();
        db.execute_unprepared(&format!(
            "INSERT INTO secretary_thread_relations \
             (relation_id, thread_id, from_event_id, to_event_id, relation_kind, \
              confidence_bps, reason, created_at) \
             VALUES ('{inbound_edge_id}', '{child_t}', \
                     '{gp_id_str}', '{child_id_str}', \
                     'same_conversation_window', 5000, \
                     '入边测试：语义边指向 child', UTC_TIMESTAMP(6))"
        ))
        .await
        .map_err(|e| format!("seed inbound edge failed: {e}"))?;
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_thread_relations \
                 WHERE to_event_id = ? AND relation_kind = 'same_conversation_window'",
                vec![child_id_str.clone().into()],
            )
            .await,
            1,
            "前置：入边必须存在"
        );

        // 父后到：触发 resolve_pending_replies_in_txn → 失效 child 旧投影 →
        // 删除全部关系边（入边+出边）。
        let parent_id = store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-18",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                Some("gp-18"),
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e}"))?
            .source_event_id()
            .clone();

        assert_eq!(
            resolved_parent_of(&db, child_id.as_str()).await.as_deref(),
            Some(parent_id.as_str()),
            "父到达后必须实时解析 child"
        );

        // 验证：child 的关系边全部清理（from 和 to 方向）。
        let cid = child_id.as_str();
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_thread_relations \
                 WHERE from_event_id = ? OR to_event_id = ?",
                vec![cid.into(), cid.into()],
            )
            .await,
            0,
            "child 迁移后所有边（入边+出边）必须被删除"
        );
        Ok(())
    })
    .await;
}

// ── 19. Reconcile fencing：过期/伪造令牌被拒绝（Codex 第四轮复核 #1）───
// resolve_claimed_pending_reply 必须用 query_one_raw FOR UPDATE 复验租约；
// 令牌不匹配或过期时返回 false，不得继续处理。

#[tokio::test]
#[ignore]
async fn reconcile_fencing_rejects_stale_or_expired_token() {
    run_scenario("_evt007s19", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let reconcile_store = build_mysql_reply_reconcile_store(db.clone());

        // 插入一个 unresolved 子事件。
        store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "c-19",
                ConversationKind::Group,
                "g-1",
                "bob",
                101,
                Some("p-19"),
            ))
            .await
            .map_err(|e| format!("insert child failed: {e}"))?;

        // 领取候选。
        let claimed = reconcile_store
            .claim_reconcile_batch(60, 1)
            .await
            .map_err(|e| format!("claim failed: {e}"))?;
        assert_eq!(claimed.len(), 1, "必须领取到一个候选");

        // 攻击者用伪造的 lease_token 尝试处理 → 必须 fail-closed。
        let mut forged = claimed[0].clone();
        forged.lease_token = uuid::Uuid::new_v4().to_string();
        assert!(
            !reconcile_store
                .resolve_claimed_pending_reply(&forged, 1000, 120_000)
                .await
                .map_err(|e| format!("resolve with forged token failed: {e}"))?,
            "伪造令牌必须被拒绝（fencing fail-closed）"
        );

        // 验证：候选行未被伪造令牌清理（主路径仍可处理）。
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_reply_reconcile_claims \
                 WHERE source_event_id = ?",
                vec![claimed[0].source_event_id.as_str().into()],
            )
            .await,
            1,
            "候选行不得被伪造令牌清理"
        );

        // 过期令牌：将租约设为已过期，再尝试处理 → 必须返回 false。
        db.execute_unprepared(&format!(
            "UPDATE secretary_reply_reconcile_claims \
             SET lease_expires_at = UTC_TIMESTAMP(6) - INTERVAL 60 SECOND \
             WHERE source_event_id = '{}'",
            claimed[0].source_event_id.as_str()
        ))
        .await
        .map_err(|e| format!("expire lease failed: {e}"))?;
        assert!(
            !reconcile_store
                .resolve_claimed_pending_reply(&claimed[0], 1000, 120_000)
                .await
                .map_err(|e| format!("resolve with expired token failed: {e}"))?,
            "过期令牌必须被拒绝（fencing fail-closed）"
        );

        Ok(())
    })
    .await;
}

// ── 20. 候选队列属性：有界、非 Reply 不入队、主路径清理（Codex 第四轮复核 #5）─

#[tokio::test]
#[ignore]
async fn candidate_queue_bounded_non_reply_excluded_mainline_cleanup() {
    run_scenario("_evt007s20", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());
        let reconcile_store = build_mysql_reply_reconcile_store(db.clone());

        // 插入 5 个 unresolved Reply 子事件 + 2 个普通事件（非 Reply）。
        for id in ["c-20a", "c-20b", "c-20c", "c-20d", "c-20e"] {
            store
                .insert_message_if_absent(&envelope(
                    "acc-a",
                    id,
                    ConversationKind::Group,
                    "g-1",
                    "bob",
                    101,
                    Some("p-20"),
                ))
                .await
                .map_err(|e| format!("insert child {id} failed: {e}"))?;
        }
        for id in ["m-20a", "m-20b"] {
            store
                .insert_message_if_absent(&envelope(
                    "acc-a",
                    id,
                    ConversationKind::Group,
                    "g-1",
                    "alice",
                    100,
                    None,
                ))
                .await
                .map_err(|e| format!("insert msg {id} failed: {e}"))?;
        }

        // 验证：非 Reply 消息不在候选队列中。
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_reply_reconcile_claims",
                Vec::new(),
            )
            .await,
            5,
            "恰好 5 个 Reply 候选入队，非 Reply 消息不得入队"
        );

        // 有界领取：batch_size=2。
        let claimed = reconcile_store
            .claim_reconcile_batch(60, 2)
            .await
            .map_err(|e| format!("claim failed: {e}"))?;
        assert_eq!(claimed.len(), 2, "有界领取不得超过 batch_size=2");

        // 父到达：主路径解析全部 pending 并清理所有候选行。
        store
            .insert_message_if_absent(&envelope(
                "acc-a",
                "p-20",
                ConversationKind::Group,
                "g-1",
                "alice",
                100,
                None,
            ))
            .await
            .map_err(|e| format!("insert parent failed: {e}"))?;
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events \
                 WHERE reply_to_platform_event_id = 'p-20' AND reply_to_event_id IS NULL",
                Vec::new(),
            )
            .await,
            0,
            "主路径必须解析所有 pending"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_reply_reconcile_claims",
                Vec::new(),
            )
            .await,
            0,
            "主路径解析必须清理全部候选行"
        );

        // migration 幂等：空回填不产生副作用。
        db.execute_unprepared(
            "INSERT IGNORE INTO secretary_reply_reconcile_claims (source_event_id) \
             SELECT source_event_id FROM secretary_source_events \
             WHERE reply_to_platform_event_id IS NOT NULL AND reply_to_event_id IS NULL",
        )
        .await
        .map_err(|e| format!("idempotent migration replay failed: {e}"))?;
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_reply_reconcile_claims",
                Vec::new(),
            )
            .await,
            0,
            "幂等回放不产生新候选行"
        );

        // 完整 migration 正向重放：删除 migration record 后由真实加载器重新执行，
        // 只有全部 SQL 成功才恢复 migration record。
        db.execute_unprepared(
            "DELETE FROM qqbot_test_schema_migrations \
             WHERE migration_name = '20260804_qqbot_reply_reconcile.sql'",
        )
        .await
        .map_err(|e| format!("delete migration record failed: {e}"))?;
        common::try_replay_folded_migration(&db, "20260804_qqbot_reply_reconcile.sql")
            .await
            .map_err(|e| format!("positive migration replay failed: {e}"))?;
        // 重放后仍无候选行（所有 pending 已解析）。
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_reply_reconcile_claims",
                Vec::new(),
            )
            .await,
            0,
            "完整 migration 重放后表结构与候选行仍一致"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM qqbot_test_schema_migrations \
                 WHERE migration_name = '20260804_qqbot_reply_reconcile.sql'",
                Vec::new(),
            )
            .await,
            1,
            "正向完整重放成功后必须写入 migration record"
        );

        // 错误索引顺序：列复验通过，索引复验必须以真实 SQL 错误中止。
        db.execute_unprepared(
            "DELETE FROM qqbot_test_schema_migrations \
             WHERE migration_name = '20260804_qqbot_reply_reconcile.sql'",
        )
        .await
        .map_err(|e| format!("delete migration record before negative replays failed: {e}"))?;
        db.execute_unprepared(
            "ALTER TABLE secretary_reply_reconcile_claims \
             DROP INDEX idx_secretary_reply_reconcile_eligible, \
             ADD KEY idx_secretary_reply_reconcile_eligible \
               (next_eligible_at, lease_expires_at, source_event_id)",
        )
        .await
        .map_err(|e| format!("install wrong reconcile index order failed: {e}"))?;
        let index_error =
            common::try_replay_folded_migration(&db, "20260804_qqbot_reply_reconcile.sql")
                .await
                .expect_err("wrong reconcile index order must fail the complete migration");
        assert_migration_schema_error(&index_error, 3, "索引顺序");
        assert_eq!(
            reply_reconcile_migration_record_count(&db).await,
            0,
            "索引结构错误导致迁移失败时不得写入 migration record"
        );

        // 恢复索引后破坏 FK 删除规则：前两项复验通过，FK 复验必须失败。
        db.execute_unprepared(
            "ALTER TABLE secretary_reply_reconcile_claims \
             DROP INDEX idx_secretary_reply_reconcile_eligible, \
             ADD KEY idx_secretary_reply_reconcile_eligible \
               (lease_expires_at, next_eligible_at, source_event_id), \
             DROP FOREIGN KEY fk_secretary_reconcile_claim_source",
        )
        .await
        .map_err(|e| format!("drop reconcile FK before wrong rule failed: {e}"))?;
        db.execute_unprepared(
            "ALTER TABLE secretary_reply_reconcile_claims \
             ADD CONSTRAINT fk_secretary_reconcile_claim_source \
             FOREIGN KEY (source_event_id) REFERENCES secretary_source_events (source_event_id) \
             ON DELETE RESTRICT",
        )
        .await
        .map_err(|e| format!("install wrong reconcile FK rule failed: {e}"))?;
        let fk_error =
            common::try_replay_folded_migration(&db, "20260804_qqbot_reply_reconcile.sql")
                .await
                .expect_err("wrong reconcile FK rule must fail the complete migration");
        assert_migration_schema_error(&fk_error, 4, "FK 删除规则");
        assert_eq!(
            reply_reconcile_migration_record_count(&db).await,
            0,
            "FK 结构错误导致迁移失败时不得写入 migration record"
        );

        // 恢复 FK 后破坏关键列长度：第一项列复验必须失败。
        db.execute_unprepared(
            "ALTER TABLE secretary_reply_reconcile_claims \
             DROP FOREIGN KEY fk_secretary_reconcile_claim_source",
        )
        .await
        .map_err(|e| format!("drop reconcile FK before restore failed: {e}"))?;
        db.execute_unprepared(
            "ALTER TABLE secretary_reply_reconcile_claims \
             ADD CONSTRAINT fk_secretary_reconcile_claim_source \
               FOREIGN KEY (source_event_id) REFERENCES secretary_source_events (source_event_id) \
               ON DELETE CASCADE, \
             MODIFY last_error VARCHAR(511) COLLATE utf8mb4_unicode_ci DEFAULT NULL",
        )
        .await
        .map_err(|e| format!("restore FK and install wrong column length failed: {e}"))?;
        let column_error =
            common::try_replay_folded_migration(&db, "20260804_qqbot_reply_reconcile.sql")
                .await
                .expect_err("wrong reconcile column length must fail the complete migration");
        assert_migration_schema_error(&column_error, 2, "列结构");
        assert_eq!(
            reply_reconcile_migration_record_count(&db).await,
            0,
            "列结构错误导致迁移失败时不得写入 migration record"
        );
        Ok(())
    })
    .await;
}

// ── 21. 真实 group_upload 关联模型：Reply 指向历史 file 消息，而非 notice file.id ──

#[tokio::test]
#[ignore]
async fn file_history_parent_resolves_delayed_reply_without_notice_source_event() {
    run_scenario("_evt007s21", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());

        // 实时子消息先到：它只有历史 file 消息 ID，不能引用 group_upload notice 的 file.id。
        let child = store
            .insert_message_if_absent(&envelope(
                "acc-file",
                "child-file-reply",
                ConversationKind::Group,
                "g-file",
                "actor-child",
                200,
                Some("history-file-parent"),
            ))
            .await
            .map_err(|error| format!("insert Reply child failed: {error}"))?;
        let child_id = child.source_event_id().as_str().to_owned();
        assert!(
            is_pending(&db, &child_id).await,
            "child must remain pending before history parent"
        );

        // 回补历史带来可引用的 file 消息父节点；不插入任何 group_upload notice SourceEvent。
        let parent = store
            .insert_message_if_absent(&file_envelope(
                "acc-file",
                "history-file-parent",
                "g-file",
                "actor-file",
                100,
            ))
            .await
            .map_err(|error| format!("insert file history parent failed: {error}"))?;
        let parent_id = parent.source_event_id().as_str().to_owned();

        assert_eq!(
            resolved_parent_of(&db, &child_id).await.as_deref(),
            Some(parent_id.as_str()),
            "file history parent must resolve the pending Reply in the same conversation"
        );
        assert!(
            !is_pending(&db, &child_id).await,
            "resolved file Reply must leave no pending state"
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events \
                 WHERE platform_event_id = 'group-upload-notice-file-id'",
                Vec::new(),
            )
            .await,
            0,
            "non-message upload notices must never be fabricated as SourceEvent parents"
        );
        Ok(())
    })
    .await;
}

// ── 22. 真实 Ark/JSON 卡片模型：历史卡片本身有稳定 ID，复用消息 Reply 解析 ──

#[tokio::test]
#[ignore]
async fn rich_card_history_parent_resolves_delayed_reply_as_message_event() {
    run_scenario("_evt007s22", |db| async move {
        let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
            build_mysql_inbound_event_store(db.clone());

        let child = store
            .insert_message_if_absent(&envelope(
                "acc-card",
                "child-card-reply",
                ConversationKind::Group,
                "g-card",
                "actor-child",
                200,
                Some("history-card-parent"),
            ))
            .await
            .map_err(|error| format!("insert card Reply child failed: {error}"))?;
        let child_id = child.source_event_id().as_str().to_owned();
        assert!(is_pending(&db, &child_id).await);

        let parent = store
            .insert_message_if_absent(&rich_card_envelope(
                "acc-card",
                "history-card-parent",
                "g-card",
                "actor-card",
                100,
            ))
            .await
            .map_err(|error| format!("insert rich card history parent failed: {error}"))?;
        let parent_id = parent.source_event_id().as_str().to_owned();

        assert_eq!(
            resolved_parent_of(&db, &child_id).await.as_deref(),
            Some(parent_id.as_str())
        );
        assert!(!is_pending(&db, &child_id).await);
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_source_events WHERE event_type = 'message'",
                Vec::new(),
            )
            .await,
            2,
            "card parent and Reply child must both remain ordinary message SourceEvents"
        );
        Ok(())
    })
    .await;
}
