//! Action Planner MySQL 集成测试。覆盖完整闭环：
//! OwnerCommand -> action_run -> claim -> Retriever -> Planner -> Effect ->
//! OwnerResponseDraft -> restart -> 不重复运行、不重复响应。
//!
//! 测试需要 QQBOT_TEST_DATABASE_URL 指向隔离 MySQL schema，默认 #[ignore]。

use std::sync::Arc;

use async_trait::async_trait;
use personal_secretary::{
    ActionPlannerT, ActionRunId, ActionRunSeed, CheckpointStore, Clock, ContentTrustLevel,
    ConversationKind, ConversationRef, EventKind, InMemoryCheckpointStore, InboundMessageEnvelope,
    IngestMessageOutcome, MatchField, MessageSource, NotificationCategory, NotificationOutcome,
    NotificationPolicyUseCase, PlannerError, PlannerInput, PlannerOutput, PlannerUseCase,
    RecentEventRef, RetrieverPolicy, RetrieverUseCase, SecretaryAction, SecretaryActionProposal,
    SecretaryAgentState, SourceAccountRef, SourceMessageRef, StructuredImportance, SystemClock,
    VerifiedActor, VerifiedActorKind, build_mysql_action_store, build_mysql_inbound_event_store,
    build_mysql_notification_policy_store, build_mysql_retriever_store,
};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};

#[path = "../../../apps/qqbot-server/database/test_support/qqbot_migrations.rs"]
mod qqbot_migrations;

async fn apply_qqbot_migrations(db: &sea_orm::DatabaseConnection) {
    qqbot_migrations::apply_qqbot_migrations(
        db,
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/qqbot-server/database/migrations"),
    )
    .await;
}

/// 保守 Planner：固定返回 NoAction，不调用 LLM。
/// 用于验证完整闭环而不依赖外部 LLM。
struct NoopPlanner;

#[async_trait]
impl ActionPlannerT for NoopPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::NoAction {
            reason: "测试用 NoAction 规划器".into(),
        })
    }
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn mysql_retriever_content_trust_matrix_and_account_isolation() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let account_id = format!("trust-{suffix}");
    let inbound_store = build_mysql_inbound_event_store(db.clone());
    let outcome = inbound_store
        .insert_message_if_absent(&owner_command(&account_id, "trust-msg", "机密正文"))
        .await
        .unwrap();
    let source_event_id = outcome.source_event_id().clone();
    let account = SourceAccountRef::new(MessageSource::QqOpenPlatform, &account_id).unwrap();
    let retriever = RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    );

    let cases = [
        ("normal", "normal", ContentTrustLevel::Normal, true),
        ("normal", "local_only", ContentTrustLevel::LocalOnly, true),
        ("local_only", "normal", ContentTrustLevel::LocalOnly, true),
        (
            "normal",
            "envelope_only",
            ContentTrustLevel::EnvelopeOnly,
            false,
        ),
        (
            "envelope_only",
            "normal",
            ContentTrustLevel::EnvelopeOnly,
            false,
        ),
        (
            "envelope_only",
            "never_long_term",
            ContentTrustLevel::NeverLongTerm,
            false,
        ),
        (
            "never_long_term",
            "normal",
            ContentTrustLevel::NeverLongTerm,
            false,
        ),
    ];
    for (conversation_mode, message_mode, expected_trust, body_visible) in cases {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_conversations c INNER JOIN secretary_accounts a ON a.id = c.account_id SET c.memory_mode = ? WHERE a.source_channel = ? AND a.platform_account_id = ?",
            vec![
                conversation_mode.into(),
                MessageSource::QqOpenPlatform.as_str().into(),
                account_id.clone().into(),
            ],
        ))
        .await
        .expect("conversation mode update must succeed");
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_message_contents SET content_mode = ? WHERE source_event_id = ?",
            vec![message_mode.into(), source_event_id.as_str().into()],
        ))
        .await
        .expect("message mode update must succeed");

        let detail = retriever
            .read_source_event(&source_event_id, &account)
            .await
            .expect("read_source_event must succeed")
            .expect("same-account event must exist");
        assert_eq!(detail.content_trust_level, expected_trust);
        assert_eq!(
            !detail.normalized_text.is_empty(),
            body_visible,
            "conversation={conversation_mode}, message={message_mode}"
        );
    }

    let other_account_id = format!("trust-other-{suffix}");
    let other_outcome = inbound_store
        .insert_message_if_absent(&owner_command(
            &other_account_id,
            "other-msg",
            "另一个账号的正文",
        ))
        .await
        .expect("other account event must persist");
    let other_source_event_id = other_outcome.source_event_id().clone();
    let wrong_account =
        SourceAccountRef::new(MessageSource::QqOpenPlatform, &other_account_id).unwrap();
    assert!(
        retriever
            .read_source_event(&source_event_id, &wrong_account)
            .await
            .expect("cross-account lookup must not fail")
            .is_none(),
        "跨账号不得按 source_event_id 读取正文"
    );

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_source_events WHERE source_event_id IN (?, ?)",
        vec![
            source_event_id.as_str().into(),
            other_source_event_id.as_str().into(),
        ],
    ))
    .await
    .ok();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_accounts WHERE source_channel = ? AND platform_account_id IN (?, ?)",
        vec![
            MessageSource::QqOpenPlatform.as_str().into(),
            account_id.into(),
            other_account_id.into(),
        ],
    ))
    .await
    .ok();
}

/// 测试用固定时钟，确保时间确定性。
struct FixedClock {
    now: i64,
}

impl Clock for FixedClock {
    fn now_unix_secs(&self) -> i64 {
        self.now
    }
}

async fn scalar_u64(
    db: &sea_orm::DatabaseConnection,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> u64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        sql,
        values,
    ))
    .await
    .expect("MySQL scalar query must succeed")
    .expect("MySQL scalar query must return one row")
    .try_get::<u64>("", "value")
    .expect("MySQL BIGINT UNSIGNED scalar must decode as u64")
}

async fn policy_epoch(db: &sea_orm::DatabaseConnection, account: &SourceAccountRef) -> u64 {
    scalar_u64(
        db,
        "SELECT policy_epoch AS value FROM secretary_accounts \
         WHERE source_channel = ? AND platform_account_id = ?",
        vec![
            account.channel.as_str().into(),
            account.account_id.clone().into(),
        ],
    )
    .await
}

fn owner_command(account_id: &str, message_id: &str, text: &str) -> InboundMessageEnvelope {
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::QqOpenPlatform, account_id, message_id).unwrap(),
        ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
        VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
        1_800_000_000,
        text,
        Vec::new(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn mysql_action_planner_full_lifecycle_restart_no_duplicate() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;

    let run_suffix = uuid::Uuid::new_v4().simple().to_string();
    let account_id = format!("action-test-{run_suffix}");

    // 1. 入站 OwnerCommand
    let inbound_store = build_mysql_inbound_event_store(db.clone());
    let command = owner_command(&account_id, "msg-1", "帮我查最近的消息");
    let outcome = inbound_store
        .insert_message_if_absent(&command)
        .await
        .unwrap();
    let source_event_id = match outcome {
        IngestMessageOutcome::Accepted {
            source_event_id, ..
        } => source_event_id,
        IngestMessageOutcome::Duplicate { .. } => panic!("expected accepted"),
    };

    // 2. 幂等创建 action_run（run_id 从 source_event_id 派生）
    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&source_event_id, "v1");
    let seed = ActionRunSeed {
        account: SourceAccountRef::new(MessageSource::QqOpenPlatform, &account_id).unwrap(),
        command_source_event_id: source_event_id.clone(),
        command_text: "帮我查最近的消息".into(),
        conversation_id: "owner-conv".into(),
        occurred_at_unix_secs: 1_800_000_000,
        timezone_offset_secs: 0,
        timezone: "UTC".into(),
        recent_events: vec![RecentEventRef {
            source_event_id: source_event_id.clone(),
            summary: "Owner 命令".into(),
        }],
    };
    let created1 = action_store
        .ensure_action_run(&run_id, &seed)
        .await
        .unwrap();
    assert!(created1, "首次创建应为 true");

    // 3. 重复创建应返回 false（幂等）
    let created2 = action_store
        .ensure_action_run(&run_id, &seed)
        .await
        .unwrap();
    assert!(!created2, "重复创建应为 false");

    // 4. 装配 PlannerUseCase（保守 NoopPlanner + RetrieverUseCase + MySQL CheckpointStore）
    let retriever_store = build_mysql_retriever_store(db.clone());
    let retriever = Arc::new(RetrieverUseCase::new(
        retriever_store,
        RetrieverPolicy::default(),
    ));
    let placeholder_checkpoint: Arc<dyn CheckpointStore<SecretaryAgentState>> =
        Arc::new(InMemoryCheckpointStore::new());
    let planner: Arc<dyn ActionPlannerT> = Arc::new(NoopPlanner);
    let use_case = Arc::new(
        PlannerUseCase::with_clock(
            action_store.clone(),
            planner,
            placeholder_checkpoint,
            60,
            Arc::new(FixedClock { now: 1_800_000_100 }),
        )
        .with_retriever(retriever)
        .with_checkpoint_db(db.clone()),
    );

    // 5. Worker 领取并运行（NoopPlanner 返回 NoAction，应直接完成）
    let report = use_case.run_once("test-worker").await.unwrap().unwrap();
    assert!(report.completed, "NoAction 应直接完成");
    assert!(!report.suspended);

    // 6. 再次运行应无待处理（无 None）
    let report2 = use_case.run_once("test-worker").await.unwrap();
    assert!(report2.is_none(), "已完成的 run 不应被再次领取");

    // 7. 重启模拟：重新装配 use_case（新进程），确认不重复运行
    let action_store2 = build_mysql_action_store(db.clone());
    let placeholder_checkpoint2: Arc<
        dyn personal_secretary::CheckpointStore<personal_secretary::SecretaryAgentState>,
    > = Arc::new(personal_secretary::InMemoryCheckpointStore::new());
    let retriever2 = Arc::new(RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    ));
    let use_case2 = Arc::new(
        PlannerUseCase::with_clock(
            action_store2,
            Arc::new(NoopPlanner) as Arc<dyn ActionPlannerT>,
            placeholder_checkpoint2,
            60,
            Arc::new(FixedClock { now: 1_800_000_200 }),
        )
        .with_retriever(retriever2)
        .with_checkpoint_db(db.clone()),
    );
    let report3 = use_case2.run_once("test-worker-restart").await.unwrap();
    assert!(report3.is_none(), "重启后不应重复处理已完成的 run");

    // 清理测试数据
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_action_runs WHERE run_id = ?",
        vec![run_id.as_str().into()],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_source_events WHERE account_id IN (SELECT id FROM secretary_accounts WHERE platform_account_id = ?)",
        vec![account_id.clone().into()],
    ))
    .await
    .ok();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_accounts WHERE platform_account_id = ?",
        vec![account_id.into()],
    ))
    .await
    .ok();
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn mysql_action_planner_lease_expiry_allows_reclaim() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;

    let run_suffix = uuid::Uuid::new_v4().simple().to_string();
    let account_id = format!("lease-test-{run_suffix}");

    let inbound_store = build_mysql_inbound_event_store(db.clone());
    let command = owner_command(&account_id, "msg-1", "查询");
    let outcome = inbound_store
        .insert_message_if_absent(&command)
        .await
        .unwrap();
    let source_event_id = match outcome {
        IngestMessageOutcome::Accepted {
            source_event_id, ..
        } => source_event_id,
        _ => panic!("expected accepted"),
    };

    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&source_event_id, "v1");
    let seed = ActionRunSeed {
        account: SourceAccountRef::new(MessageSource::QqOpenPlatform, &account_id).unwrap(),
        command_source_event_id: source_event_id.clone(),
        command_text: "查询".into(),
        conversation_id: "owner-conv".into(),
        occurred_at_unix_secs: 1_800_000_000,
        timezone_offset_secs: 0,
        timezone: "UTC".into(),
        recent_events: Vec::new(),
    };
    action_store
        .ensure_action_run(&run_id, &seed)
        .await
        .unwrap();

    // 用极短租约领取（1 秒），模拟 Worker 崩溃
    let planner: Arc<dyn ActionPlannerT> = Arc::new(NoopPlanner);
    let placeholder_cp: Arc<
        dyn personal_secretary::CheckpointStore<personal_secretary::SecretaryAgentState>,
    > = Arc::new(personal_secretary::InMemoryCheckpointStore::new());
    let _use_case = Arc::new(
        PlannerUseCase::with_clock(
            action_store.clone(),
            planner,
            placeholder_cp,
            1, // 1 秒租约
            Arc::new(FixedClock { now: 1_800_000_000 }),
        )
        .with_checkpoint_db(db.clone()),
    );

    // 手动领取一个 run（模拟 Worker 领取后崩溃，不执行 run_once）
    let claimed = action_store
        .claim_pending_run("crashed-worker", 1, 1_800_000_000)
        .await
        .unwrap();
    assert!(claimed.is_some(), "应能领取 pending run");

    // 等待租约过期
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // 过期后应能被其他 Worker 重新领取（claim_pending_run 内部回收过期租约）
    let reclaimed = action_store
        .claim_pending_run("new-worker", 60, 1_800_000_002)
        .await
        .unwrap();
    assert!(reclaimed.is_some(), "过期租约应被回收，允许重新领取");

    // 清理
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_action_runs WHERE run_id = ?",
        vec![run_id.as_str().into()],
    ))
    .await
    .ok();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_source_events WHERE account_id IN (SELECT id FROM secretary_accounts WHERE platform_account_id = ?)",
        vec![account_id.clone().into()],
    ))
    .await
    .ok();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_accounts WHERE platform_account_id = ?",
        vec![account_id.into()],
    ))
    .await
    .ok();
}

/// 生成 SearchRecentEvents Proposal 的 Planner，用于测试 Effect 真正执行 Retriever。
struct SearchActionPlanner;

#[async_trait]
impl ActionPlannerT for SearchActionPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            personal_secretary::SecretaryActionProposal::new(
                personal_secretary::SecretaryAction::SearchRecentEvents {
                    query: "测试".into(),
                    limit: 20,
                },
                "测试检索",
                Vec::new(),
                None,
            )
            .map_err(|e| PlannerError::InvalidOutput(e.to_string()))?,
        ))
    }
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn mysql_action_planner_retriever_effect_response_roundtrip() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;

    let run_suffix = uuid::Uuid::new_v4().simple().to_string();
    let account_id = format!("reteff-{run_suffix}");

    // 1. 入站 OwnerCommand
    let inbound_store = build_mysql_inbound_event_store(db.clone());
    let command = owner_command(&account_id, "msg-1", "帮我查测试");
    let outcome = inbound_store
        .insert_message_if_absent(&command)
        .await
        .unwrap();
    let source_event_id = match outcome {
        IngestMessageOutcome::Accepted {
            source_event_id, ..
        } => source_event_id,
        _ => panic!("expected accepted"),
    };

    // 2. 创建 action_run
    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&source_event_id, "v1");
    let seed = ActionRunSeed {
        account: SourceAccountRef::new(MessageSource::QqOpenPlatform, &account_id).unwrap(),
        command_source_event_id: source_event_id.clone(),
        command_text: "帮我查测试".into(),
        conversation_id: "owner-conv".into(),
        occurred_at_unix_secs: 1_800_000_000,
        timezone_offset_secs: 0,
        timezone: "UTC".into(),
        recent_events: Vec::new(),
    };
    action_store
        .ensure_action_run(&run_id, &seed)
        .await
        .unwrap();

    // 3. 装配 PlannerUseCase（SearchActionPlanner → SearchRecentEvents Effect 执行 Retriever）
    let retriever = Arc::new(RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    ));
    let placeholder_cp: Arc<dyn CheckpointStore<SecretaryAgentState>> =
        Arc::new(InMemoryCheckpointStore::new());
    let use_case = Arc::new(
        PlannerUseCase::with_clock(
            action_store.clone(),
            Arc::new(SearchActionPlanner) as Arc<dyn ActionPlannerT>,
            placeholder_cp,
            60,
            Arc::new(FixedClock { now: 1_800_000_100 }),
        )
        .with_retriever(retriever)
        .with_checkpoint_db(db.clone()),
    );

    // 4. 运行：Proposal → Effect 调 Retriever → 真实 result_ref
    let report = use_case.run_once("test-worker").await.unwrap().unwrap();
    assert!(report.completed, "SearchRecentEvents 应完成执行");
    assert!(!report.suspended);

    // 5. 重复领取应返回 None
    assert!(use_case.run_once("test-worker").await.unwrap().is_none());

    // 6. 验证 effect_receipt 已写入且含真实 Retriever 结果（非 executed: 前缀）。
    let count = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS cnt FROM secretary_action_effect_receipts WHERE run_id = ?",
            vec![run_id.as_str().into()],
        ))
        .await
        .expect("COUNT query must succeed")
        .expect("COUNT 查询必须返回行（COUNT(*) 总是返回一行）");
    let count: u64 = count.try_get_by_index(0).expect("COUNT(*) 必须按 u64 解码");
    assert_eq!(count, 1, "应恰好有 1 条 effect_receipt");

    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT result_ref FROM secretary_action_effect_receipts WHERE run_id = ? LIMIT 1",
            vec![run_id.as_str().into()],
        ))
        .await
        .expect("SELECT result_ref must succeed")
        .expect("effect_receipt 行必须存在");
    let result_ref: String = row
        .try_get_by_index::<String>(0)
        .expect("result_ref 列必须可读");
    assert!(!result_ref.is_empty(), "result_ref 不应为空");
    assert!(
        !result_ref.starts_with("executed:"),
        "result_ref 不应是伪造的 executed: 前缀，应为 Retriever 真实结果: {result_ref}"
    );
    assert!(
        result_ref.contains("命中 1 条")
            && result_ref.contains(source_event_id.as_str())
            && result_ref.contains("owner-openid"),
        "result_ref 应包含稳定的命中数、来源事件和 Actor: {result_ref}"
    );

    let response_count = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(COUNT(*) AS UNSIGNED) FROM secretary_action_responses WHERE run_id = ?",
            vec![run_id.as_str().into()],
        ))
        .await
        .expect("response COUNT query must succeed")
        .expect("response COUNT must return a row")
        .try_get_by_index::<u64>(0)
        .expect("response COUNT must decode as u64");
    assert_eq!(response_count, 1, "完成后必须持久化唯一响应产物");

    // 清理
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_action_effect_receipts WHERE run_id = ?",
        vec![run_id.as_str().into()],
    ))
    .await
    .ok();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_action_runs WHERE run_id = ?",
        vec![run_id.as_str().into()],
    ))
    .await
    .ok();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_source_events WHERE account_id IN (SELECT id FROM secretary_accounts WHERE platform_account_id = ?)",
        vec![account_id.clone().into()],
    ))
    .await
    .ok();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_accounts WHERE platform_account_id = ?",
        vec![account_id.into()],
    ))
    .await
    .ok();
}

/// 生成需审批的通知策略 Proposal，用于验证重启后执行真实 MySQL effect。
struct SuspendPolicyPlanner {
    account: SourceAccountRef,
}

#[async_trait]
impl ActionPlannerT for SuspendPolicyPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::SetNotificationCategoryImportance {
                    canonical_scope_key: "category:agenda".into(),
                    match_key: personal_secretary::NotificationMatchKeyV1::new(
                        self.account.clone(),
                        MatchField::Absent,
                        MatchField::Absent,
                        MatchField::Known(NotificationCategory::Agenda),
                        MatchField::Known(false),
                        MatchField::Known(StructuredImportance::Normal),
                        MatchField::Known(EventKind::AgendaDue),
                    )
                    .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
                    outcome: NotificationOutcome::Suppress,
                    bypass_quiet: false,
                },
                "测试重启后确认通知策略变更",
                Vec::new(),
                Some("resume-policy-effect-v1".into()),
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn mysql_action_planner_restart_resume_approved_policy_effect_once() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let managed_account =
        SourceAccountRef::new(MessageSource::NapCat, format!("resume-policy-{suffix}"))
            .expect("valid managed account");
    let command_account_id = format!("resume-command-{suffix}");
    let inbound_store = build_mysql_inbound_event_store(db.clone());
    inbound_store
        .begin_connection(&managed_account)
        .await
        .expect("managed account bootstrap must succeed");
    let source_event_id = inbound_store
        .insert_message_if_absent(&owner_command(
            &command_account_id,
            "msg-1",
            "确认创建日程提醒策略",
        ))
        .await
        .expect("owner command must persist")
        .source_event_id()
        .clone();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_owner_bindings \
         (binding_id, managed_account_id, command_account_id, owner_actor_id, status) \
         SELECT ?, managed.id, command.id, 'owner-openid', 'active' \
         FROM secretary_accounts AS managed CROSS JOIN secretary_accounts AS command \
         WHERE managed.source_channel = ? AND managed.platform_account_id = ? \
           AND command.source_channel = 'qq_open_platform' AND command.platform_account_id = ?",
        vec![
            uuid::Uuid::new_v4().to_string().into(),
            managed_account.channel.as_str().into(),
            managed_account.account_id.clone().into(),
            command_account_id.clone().into(),
        ],
    ))
    .await
    .expect("cross-account owner binding must persist");

    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&source_event_id, "resume-policy-effect-v1");
    action_store
        .ensure_action_run(
            &run_id,
            &ActionRunSeed {
                account: managed_account.clone(),
                command_source_event_id: source_event_id.clone(),
                command_text: "确认创建日程提醒策略".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: 1_800_000_000,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![RecentEventRef {
                    source_event_id: source_event_id.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .expect("policy action run must persist");
    let before_epoch = policy_epoch(&db, &managed_account).await;
    let initial = PlannerUseCase::with_clock(
        action_store,
        Arc::new(SuspendPolicyPlanner {
            account: managed_account.clone(),
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
        Arc::new(FixedClock { now: 1_800_000_100 }),
    )
    .with_checkpoint_db(db.clone());

    let report = initial
        .run_once("test-worker")
        .await
        .expect("policy proposal run must succeed")
        .expect("policy proposal run must be claimed");
    assert!(report.suspended, "L2 policy action must await approval");
    let checkpoint_id = report
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal_id = report
        .proposal_id
        .expect("suspended run must have proposal");

    // 进程重建后必须重新装配 MySQL 策略用例；不能复用先前的内存对象。
    let resumed = PlannerUseCase::with_clock(
        build_mysql_action_store(db.clone()),
        Arc::new(NoopPlanner) as Arc<dyn ActionPlannerT>,
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
        Arc::new(FixedClock { now: 1_800_000_200 }),
    )
    .with_checkpoint_db(db.clone())
    .with_notification_policy(Arc::new(NotificationPolicyUseCase::new(
        build_mysql_notification_policy_store(db.clone()),
        Arc::new(SystemClock),
    )));
    let first_resume = resumed
        .resume_run(
            &run_id,
            &checkpoint_id,
            personal_secretary::SecretaryActionResumeInput {
                proposal_id: proposal_id.clone(),
                decision: personal_secretary::SecretaryApprovalDecision::Approve,
                command_source_event_id: source_event_id.clone(),
                approval_source_event_id: None,
            },
        )
        .await
        .expect("approved policy resume must execute effect");
    assert!(
        first_resume.completed,
        "approved resume must complete action run"
    );
    assert_eq!(policy_epoch(&db, &managed_account).await, before_epoch + 1);
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value \
             FROM secretary_notification_policy_revisions AS revision \
             INNER JOIN secretary_notification_policy_families AS family \
               ON family.policy_family_id = revision.policy_family_id \
             INNER JOIN secretary_accounts AS account ON account.id = family.account_id \
             WHERE account.source_channel = ? AND account.platform_account_id = ?",
            vec![
                managed_account.channel.as_str().into(),
                managed_account.account_id.clone().into(),
            ],
        )
        .await,
        1,
        "approved resume must create exactly one policy revision",
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value \
             FROM secretary_action_effect_receipts WHERE run_id = ?",
            vec![run_id.as_str().into()],
        )
        .await,
        1,
        "approved resume must persist exactly one effect receipt",
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value \
             FROM secretary_action_responses WHERE run_id = ?",
            vec![run_id.as_str().into()],
        )
        .await,
        1,
        "completed resume must persist one response",
    );

    assert!(
        resumed
            .resume_run(
                &run_id,
                &checkpoint_id,
                personal_secretary::SecretaryActionResumeInput {
                    proposal_id,
                    decision: personal_secretary::SecretaryApprovalDecision::Approve,
                    command_source_event_id: source_event_id,
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "checkpoint CAS must reject the second approved resume",
    );
    assert_eq!(policy_epoch(&db, &managed_account).await, before_epoch + 1);
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value \
             FROM secretary_action_effect_receipts WHERE run_id = ?",
            vec![run_id.as_str().into()],
        )
        .await,
        1,
        "rejected second resume must not create another effect receipt",
    );
}

/// 生成 Owner 澄清请求的 Planner，用于测试允许动作上的 Suspend→Resume 基础设施。
struct SuspendPlanner;

#[async_trait]
impl ActionPlannerT for SuspendPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Clarification {
            question: "请确认是否继续处理测试事项".into(),
            evidence: Vec::new(),
        })
    }
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn mysql_action_planner_suspend_resume_cas_single_consume() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;

    let run_suffix = uuid::Uuid::new_v4().simple().to_string();
    let account_id = format!("susp-{run_suffix}");

    let inbound_store = build_mysql_inbound_event_store(db.clone());
    let command = owner_command(&account_id, "msg-1", "创建提醒");
    let outcome = inbound_store
        .insert_message_if_absent(&command)
        .await
        .unwrap();
    let source_event_id = match outcome {
        IngestMessageOutcome::Accepted {
            source_event_id, ..
        } => source_event_id,
        _ => panic!("expected accepted"),
    };

    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&source_event_id, "v1");
    let seed = ActionRunSeed {
        account: SourceAccountRef::new(MessageSource::QqOpenPlatform, &account_id).unwrap(),
        command_source_event_id: source_event_id.clone(),
        command_text: "创建提醒".into(),
        conversation_id: "owner-conv".into(),
        occurred_at_unix_secs: 1_800_000_000,
        timezone_offset_secs: 0,
        timezone: "UTC".into(),
        recent_events: Vec::new(),
    };
    action_store
        .ensure_action_run(&run_id, &seed)
        .await
        .unwrap();

    let retriever = Arc::new(RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    ));
    let placeholder_cp: Arc<dyn CheckpointStore<SecretaryAgentState>> =
        Arc::new(InMemoryCheckpointStore::new());
    let use_case = Arc::new(
        PlannerUseCase::with_clock(
            action_store.clone(),
            Arc::new(SuspendPlanner) as Arc<dyn ActionPlannerT>,
            placeholder_cp,
            60,
            Arc::new(FixedClock { now: 1_800_000_100 }),
        )
        .with_retriever(retriever)
        .with_checkpoint_db(db.clone()),
    );

    // 运行 → 应挂起（Owner 澄清是本批允许的 ExternalInput 挂起路径）
    let report = use_case.run_once("test-worker").await.unwrap().unwrap();
    assert!(report.suspended, "Owner 澄清请求应挂起等待输入");
    let checkpoint_id = report.checkpoint_id.expect("挂起时应返回 checkpoint_id");
    let proposal_id = report.proposal_id.expect("挂起时应返回 proposal_id");
    let suspended_row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT status, lease_token FROM secretary_action_runs WHERE run_id = ?",
            vec![run_id.as_str().into()],
        ))
        .await
        .expect("suspended run query must succeed")
        .expect("suspended run must exist");
    assert_eq!(
        suspended_row
            .try_get_by_index::<String>(0)
            .expect("status must decode"),
        "suspended"
    );
    assert!(
        suspended_row
            .try_get_by_index::<Option<String>>(1)
            .expect("lease_token must decode")
            .is_none(),
        "挂起后必须释放 Worker 租约"
    );

    // 模拟重启：新建 use_case，Resume → CAS 单次消费
    let new_use_case = Arc::new(
        PlannerUseCase::with_clock(
            build_mysql_action_store(db.clone()),
            Arc::new(NoopPlanner) as Arc<dyn ActionPlannerT>,
            Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
            60,
            Arc::new(FixedClock { now: 1_800_000_200 }),
        )
        .with_checkpoint_db(db.clone()),
    );
    let wrong_proposal = new_use_case
        .resume_run(
            &run_id,
            &checkpoint_id,
            personal_secretary::SecretaryActionResumeInput {
                proposal_id: "wrong-proposal".into(),
                decision: personal_secretary::SecretaryApprovalDecision::Reject,
                command_source_event_id: source_event_id.clone(),
                approval_source_event_id: None,
            },
        )
        .await;
    assert!(
        wrong_proposal.is_err(),
        "错误 proposal_id 不得领取恢复租约或消费 Checkpoint"
    );
    let resume_input = personal_secretary::SecretaryActionResumeInput {
        proposal_id: proposal_id.clone(),
        decision: personal_secretary::SecretaryApprovalDecision::Reject,
        command_source_event_id: source_event_id.clone(),
        approval_source_event_id: None,
    };
    let resume_report = new_use_case
        .resume_run(&run_id, &checkpoint_id, resume_input)
        .await;
    assert!(
        resume_report.is_ok(),
        "Resume 应成功: {:?}",
        resume_report.err()
    );

    // 第二次 Resume 应被 CAS 单次消费拒绝
    let resume2 = new_use_case
        .resume_run(
            &run_id,
            &checkpoint_id,
            personal_secretary::SecretaryActionResumeInput {
                proposal_id,
                decision: personal_secretary::SecretaryApprovalDecision::Reject,
                command_source_event_id: source_event_id,
                approval_source_event_id: None,
            },
        )
        .await;
    assert!(resume2.is_err(), "第二次 Resume 应被 CAS 单次消费拒绝");

    let completed_row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT r.status, c.checkpoint_status FROM secretary_action_runs r INNER JOIN secretary_action_checkpoints c ON c.run_id = r.run_id WHERE r.run_id = ? AND c.checkpoint_id = ?",
            vec![run_id.as_str().into(), checkpoint_id.clone().into()],
        ))
        .await
        .expect("completed resume query must succeed")
        .expect("completed run and checkpoint must exist");
    assert_eq!(
        completed_row
            .try_get_by_index::<String>(0)
            .expect("run status must decode"),
        "completed"
    );
    assert_eq!(
        completed_row
            .try_get_by_index::<String>(1)
            .expect("checkpoint status must decode"),
        "consumed"
    );

    // 清理
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_action_runs WHERE run_id = ?",
        vec![run_id.as_str().into()],
    ))
    .await
    .ok();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_source_events WHERE account_id IN (SELECT id FROM secretary_accounts WHERE platform_account_id = ?)",
        vec![account_id.clone().into()],
    ))
    .await
    .ok();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_accounts WHERE platform_account_id = ?",
        vec![account_id.into()],
    ))
    .await
    .ok();
}
