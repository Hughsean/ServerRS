//! 参与者稳定身份 + 事件因果关系 + 人物上下文（ID-004/ID-005/THR-011/THR-012/MEM-002）
//! 随机隔离 MySQL 主路径。
//!
//! 场景：账号 A 中 Alice 提出要求 → Bob 回复并 @Carol → Carol 承诺处理（同一有效线程）；
//! 已确认 Request 声明与已确认承诺记忆支撑角色关系；账号 B 复用相同 actor_id/message_id
//! 证明跨账号零关联；envelope_only 来源不得支撑人物长期事实。
//!
//! 需要 QQBOT_TEST_DATABASE_URL 指向随机 `qqbot_accept_*` 隔离 schema；默认 #[ignore]。
//! 测试结束在 finally 清理 schema。

use std::sync::Arc;

use async_trait::async_trait;
use personal_secretary::{
    ActionPlannerT, ActionRunId, ActionRunSeed, CommitmentMemory, CommitmentStatus, ContentSegment,
    ConversationKind, ConversationRef, EventRelationKind, InMemoryCheckpointStore,
    InboundMessageEnvelope, IngestMessageOutcome, MessageSource, ObservedSenderProfile,
    ParticipantAttributeKind, PersonMemory, PlannerError, PlannerInput, PlannerOutput,
    PlannerUseCase, PlatformIdentityKind, RecentEventRef, RetrieverPolicy, RetrieverUseCase,
    SecretaryAction, SecretaryActionProposal, SecretaryAgentState, SourceAccountRef, SourceEventId,
    SourceMessageRef, SystemClock, ThreadActorRef, VerifiedActor, VerifiedActorKind,
};
use personal_secretary_mysql::{
    build_mysql_action_checkpoint_store_factory, build_mysql_action_store,
    build_mysql_inbound_event_store, build_mysql_retriever_store,
};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};

#[path = "../../qqbot-server/database/test_support/qqbot_migrations.rs"]
mod qqbot_migrations;

/// 同进程内多个测试不能共享同一个随机 schema（先完成的测试会在 finally 中
/// DROP DATABASE 破坏仍在运行的场景）：每个测试用 URL 基础 schema + 后缀派生
/// 独立 schema，互不干扰；需要运行环境对 `qqbot_accept_%` 模式授权
/// （GRANT ALL ON `qqbot_accept\_%`.*）。
async fn isolated_db(suffix: &str) -> (DatabaseConnection, String) {
    let base_url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must point to an isolated qqbot_accept_* schema");
    let base_schema = base_url
        .split('?')
        .next()
        .and_then(|value| value.rsplit('/').next())
        .unwrap_or_default()
        .to_owned();
    assert!(
        base_schema.starts_with("qqbot_accept_")
            && base_schema
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_'),
        "refusing to run acceptance tests against non-isolated schema: {base_schema}"
    );
    assert!(
        suffix
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    );
    let random = uuid::Uuid::new_v4().simple().to_string();
    let tail = format!("{suffix}-{}", &random[..12]);
    let max_base_len = 64usize
        .checked_sub(tail.len())
        .expect("test schema suffix must fit MySQL identifier limit");
    assert!(max_base_len >= "qqbot_accept_".len());
    let schema = format!(
        "{}{tail}",
        &base_schema[..base_schema.len().min(max_base_len)]
    );
    // 用基础连接创建派生 schema（模式授权覆盖 qqbot_accept_%）。
    let base = Database::connect(&base_url)
        .await
        .expect("connect isolated acceptance MySQL");
    base.execute_unprepared(&format!("CREATE DATABASE IF NOT EXISTS `{schema}`"))
        .await
        .expect("create derived acceptance schema");
    drop(base);
    let (prefix, _) = base_url.rsplit_once('/').expect("url must contain schema");
    let url = format!("{prefix}/{schema}");
    let db = Database::connect(url)
        .await
        .expect("connect derived isolated acceptance MySQL");
    qqbot_migrations::apply_qqbot_migrations(
        &db,
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../qqbot-server/database/migrations"),
    )
    .await;
    (db, schema)
}

const GROUP_A: &str = "g-9-3";
const GROUP_B: &str = "g-9-3-b";
const GROUP_ENV: &str = "g-9-3-env";
const ACCT_A: &str = "acct-a";
const ACCT_B: &str = "acct-b";

fn account(subject: &str) -> SourceAccountRef {
    SourceAccountRef::new(MessageSource::NapCat, subject).expect("valid account fixture")
}

fn profile(nickname: &str, card: Option<&str>, role: Option<&str>) -> ObservedSenderProfile {
    ObservedSenderProfile {
        nickname: nickname.into(),
        group_card: card.map(str::to_owned),
        group_role: role.map(str::to_owned),
    }
}

#[allow(clippy::too_many_arguments)]
fn envelope(
    account_subject: &str,
    group_id: &str,
    message_id: &str,
    actor_id: &str,
    text: &str,
    segments: Vec<ContentSegment>,
    sender_profile: Option<ObservedSenderProfile>,
    occurred_at_unix_secs: i64,
) -> InboundMessageEnvelope {
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, account_subject, message_id)
            .expect("valid source fixture"),
        ConversationRef::new(ConversationKind::Group, group_id).expect("valid group fixture"),
        VerifiedActor::new(VerifiedActorKind::External, actor_id).expect("valid actor fixture"),
        occurred_at_unix_secs,
        text,
        segments,
    )
    .expect("valid inbound fixture")
    .with_sender_profile(sender_profile)
    .expect("valid sender profile fixture")
}

/// 确定性 Planner：返回 GetEventCausalContext 提案，复现完整 PlannerUseCase 闭环。
struct CausalQueryPlanner {
    target_event_id: SourceEventId,
}

#[async_trait]
impl ActionPlannerT for CausalQueryPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::GetEventCausalContext {
                    source_event_id: self.target_event_id.clone(),
                },
                "测试查询事件因果上下文",
                vec![self.target_event_id.clone()],
                None,
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

/// 确定性 Planner：返回 GetParticipantContextByName 提案（THR-013 复合查询闭环）。
struct NameQueryPlanner {
    name: String,
}

#[async_trait]
impl ActionPlannerT for NameQueryPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::GetParticipantContextByName {
                    name: self.name.clone(),
                    conversation_ref: None,
                    thread_id: None,
                },
                "测试按名字查询参与者",
                Vec::new(),
                None,
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

/// CMD-010 防线 A：建立真实验证的 QQ 开放平台 OwnerCommand + active
/// OwnerBinding（managed = NapCat 托管账号）。ActionRun 只能由这种命令创建。
#[allow(clippy::too_many_arguments)]
async fn owner_command_with_binding_9_3(
    db: &DatabaseConnection,
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    managed: &SourceAccountRef,
    command_account_id: &str,
    message_id: &str,
    text: &str,
) -> Result<SourceEventId, String> {
    let command_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(
                    MessageSource::QqOpenPlatform,
                    command_account_id,
                    message_id,
                )
                .expect("valid command source fixture"),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv")
                    .expect("valid owner control fixture"),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid")
                    .expect("valid owner actor fixture"),
                1_800_000_090,
                text.to_string(),
                Vec::new(),
            )
            .expect("valid owner command"),
        )
        .await
        .map_err(|e| format!("owner command persist failed: {e}"))?
        .source_event_id()
        .clone();
    let binding = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT IGNORE INTO secretary_owner_bindings \
             (binding_id, managed_account_id, command_account_id, owner_actor_id, status) \
             SELECT ?, managed.id, command.id, 'owner-openid', 'active' \
             FROM secretary_accounts managed CROSS JOIN secretary_accounts command \
             WHERE managed.source_channel = 'napcat' AND managed.platform_account_id = ? \
               AND command.source_channel = 'qq_open_platform' AND command.platform_account_id = ?",
            vec![
                sea_orm::Value::String(Some(uuid::Uuid::new_v4().to_string())),
                sea_orm::Value::String(Some(managed.account_id.clone())),
                sea_orm::Value::String(Some(command_account_id.to_string())),
            ],
        ))
        .await
        .map_err(|e| format!("owner binding persist failed: {e}"))?;
    assert!(
        binding.rows_affected() <= 1,
        "binding insert is idempotent for the same Owner identity"
    );
    Ok(command_event_id)
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
        .expect("acceptance query must return one row");
    row.try_get::<u64>("", "value")
        .or_else(|_| row.try_get::<i64>("", "value").map(|v| v as u64))
        .expect("acceptance scalar must decode as integer")
}

async fn scalar_str(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> String {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .await
        .expect("acceptance query must execute")
        .expect("acceptance query must return one row");
    row.try_get::<String>("", "value")
        .expect("acceptance scalar must decode as string")
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn participant_causality_mysql_main_path() {
    // 修复预先存在缺陷：空后缀会让派生 schema 等于基础 schema，
    // finally 的 DROP 会误删共享基础库（qqbot_accept_ci），导致后续
    // 测试全部 connect 失败。改用随机派生 schema，finally 只清理自己的。
    let (db, schema) = isolated_db("_main").await;
    // 场景放入独立 task：断言 panic 时先拿到 JoinError，保证清理必然执行。
    let outcome = tokio::spawn(scenario(db.clone())).await;
    // finally：随机 schema 必须清理；清理失败属于验收基础设施失败，不能吞掉。
    db.execute_unprepared(&format!("DROP DATABASE IF EXISTS `{schema}`"))
        .await
        .expect("drop participant causality schema");
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(message)) => panic!("participant causality scenario must pass: {message}"),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
}

async fn scenario(db: DatabaseConnection) -> Result<(), String> {
    let acct_a = account(ACCT_A);
    let acct_b = account(ACCT_B);
    let inbound = build_mysql_inbound_event_store(db.clone());

    // ---- 账号 A：Alice 提出要求 → Bob 回复并 @Carol → Carol 承诺处理 ----
    let e1 = ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_A,
            "m-1",
            "alice-10001",
            "我需要下周完成报告",
            Vec::new(),
            Some(profile("Alice", Some("A-名片"), Some("owner"))),
            1_800_000_000,
        ),
    )
    .await?;
    let e2 = ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_A,
            "m-2",
            "bob-10002",
            "我来回复，@Carol 你跟进一下",
            vec![
                ContentSegment::Reply {
                    platform_message_id: "m-1".into(),
                },
                ContentSegment::Mention {
                    actor_id: "carol-10003".into(),
                },
            ],
            Some(profile("Bob", None, Some("member"))),
            1_800_000_010,
        ),
    )
    .await?;
    let e3 = ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_A,
            "m-3",
            "carol-10003",
            "我来处理",
            Vec::new(),
            Some(profile("Carol", None, Some("admin"))),
            1_800_000_020,
        ),
    )
    .await?;

    // envelope_only 会话中的敏感消息：先入库创建会话，再切换 memory_mode。
    // 信封级显示信息（与 e1 相同的档案）仍可观察；正文与人物事实不得泄漏。
    let e4 = ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_ENV,
            "m-4",
            "alice-10001",
            "信封模式下的敏感正文",
            Vec::new(),
            Some(profile("Alice", Some("A-名片"), Some("owner"))),
            1_800_000_030,
        ),
    )
    .await?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_conversations SET memory_mode = 'envelope_only' \
         WHERE conversation_kind = 'group' AND platform_conversation_id = ? \
           AND account_id = (SELECT id FROM secretary_accounts \
                             WHERE source_channel = 'napcat' AND platform_account_id = ?)",
        [GROUP_ENV.into(), ACCT_A.into()],
    ))
    .await
    .map_err(|e| format!("envelope_only conversation setup failed: {e}"))?;

    // ---- 账号 B：复用相同 actor_id 与 message_id，证明跨账号零关联 ----
    let e5 = ingest(
        inbound.as_ref(),
        envelope(
            ACCT_B,
            "g-other",
            "m-2",
            "bob-10002",
            "另一账号的同名消息",
            Vec::new(),
            None,
            2_000_000_000,
        ),
    )
    .await?;

    // ---- 确定性线程投影（生产由线程 Worker 写入；此处直接建立同一结构）----
    let account_id_a = scalar_u64(
        &db,
        "SELECT id AS value FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
        vec![ACCT_A.into()],
    )
    .await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_event_threads \
         (thread_id, account_id, status, root_event_id, latest_event_id, \
          opened_at_unix_secs, latest_occurred_at_unix_secs) \
         VALUES (?, ?, 'open', ?, ?, ?, ?)",
        [
            "th-9-3".into(),
            account_id_a.into(),
            e1.as_str().into(),
            e3.as_str().into(),
            1_800_000_000i64.into(),
            1_800_000_020i64.into(),
        ],
    ))
    .await
    .map_err(|e| format!("thread fixture failed: {e}"))?;
    for event in [&e1, &e2, &e3] {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_thread_events (source_event_id, thread_id) VALUES (?, ?)",
            [event.as_str().into(), "th-9-3".into()],
        ))
        .await
        .map_err(|e| format!("thread membership fixture failed: {e}"))?;
    }

    // ---- 已确认语义：Alice 是要求者（confirmed request 声明）；Carol 是承诺人/受益方 ----
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_claims \
         (claim_id, thread_id, claim_kind, claimant_channel, claimant_account, \
          claimant_actor_id, statement, status, confidence_bps) \
         VALUES (?, ?, 'request', 'napcat', ?, ?, ?, 'confirmed', 10000)",
        [
            "claim-9-3".into(),
            "th-9-3".into(),
            ACCT_A.into(),
            "alice-10001".into(),
            "需要下周完成报告".into(),
        ],
    ))
    .await
    .map_err(|e| format!("request claim fixture failed: {e}"))?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_claim_sources (claim_id, source_event_id) VALUES (?, ?)",
        ["claim-9-3".into(), e1.as_str().into()],
    ))
    .await
    .map_err(|e| format!("claim source fixture failed: {e}"))?;

    // Carol 的确认人物记忆（职责 + 沟通偏好），来源 e3。
    insert_person_memory(
        &db,
        "fact-carol",
        &PersonMemory {
            person: ThreadActorRef {
                platform_identity_kind: None,
                account: acct_a.clone(),
                actor_id: "carol-10003".into(),
            },
            relationship: None,
            responsibilities: vec!["负责报告整理".into()],
            communication_preferences: vec!["偏好邮件沟通".into()],
        },
        &e3,
    )
    .await?;
    // Alice 的"确认"人物记忆来源是 envelope_only 会话 —— 不得支撑任何人物事实。
    insert_person_memory(
        &db,
        "fact-alice-env",
        &PersonMemory {
            person: ThreadActorRef {
                platform_identity_kind: None,
                account: acct_a.clone(),
                actor_id: "alice-10001".into(),
            },
            relationship: None,
            responsibilities: vec!["来自信封模式的职责".into()],
            communication_preferences: Vec::new(),
        },
        &e4,
    )
    .await?;
    // Carol 的确认承诺记忆（promisor=Carol, beneficiary=Alice），来源 e3。
    insert_commitment_memory(
        &db,
        "fact-commit-carol",
        &CommitmentMemory {
            promisor: ThreadActorRef {
                platform_identity_kind: None,
                account: acct_a.clone(),
                actor_id: "carol-10003".into(),
            },
            beneficiary: ThreadActorRef {
                platform_identity_kind: None,
                account: acct_a.clone(),
                actor_id: "alice-10001".into(),
            },
            action: "处理报告".into(),
            due_at_unix_secs: None,
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        },
        &e3,
    )
    .await?;

    // ---- Retriever 用例查询 ----
    let retriever = Arc::new(RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    ));

    // 1) 事件因果上下文：Bob 的回复事件。
    let ctx = retriever
        .event_causal_context(&acct_a, &e2)
        .await
        .map_err(|e| format!("event_causal_context failed: {e}"))?
        .expect("acct-a 的 e2 必须可查询");
    assert_eq!(
        ctx.sender.as_ref().map(|s| s.stable_id.as_str()),
        Some("bob-10002"),
        "发送者必须是 Bob"
    );
    assert_eq!(
        ctx.sender.as_ref().and_then(|s| s.display_name.as_deref()),
        Some("Bob"),
        "发送者显示名来自观察档案"
    );
    let parent = ctx.reply_parent.as_ref().expect("e2 必须回复 e1");
    assert_eq!(parent.source_event_id.as_str(), e1.as_str());
    assert_eq!(
        parent.sender.as_ref().map(|s| s.stable_id.as_str()),
        Some("alice-10001"),
        "回复根发送者 = Alice，不等于当前发送者 Bob"
    );
    let thread = ctx.thread.as_ref().expect("e2 必须属于线程 th-9-3");
    assert_eq!(thread.thread_id.as_str(), "th-9-3");
    assert_eq!(thread.root_event_id.as_str(), e1.as_str());
    assert_eq!(
        thread.root_sender.as_ref().map(|s| s.stable_id.as_str()),
        Some("alice-10001"),
        "线程发起人 = 根事件发送者 Alice"
    );
    assert!(
        ctx.mentioned.iter().any(|m| m.stable_id() == "carol-10003"),
        "被@参与者必须包含 Carol"
    );
    assert_eq!(ctx.requesters.len(), 1);
    assert_eq!(
        ctx.requesters[0].stable_id(),
        "alice-10001",
        "要求者 = Alice"
    );
    assert!(ctx.assignees.is_empty(), "v1 无负责人生产者，未知即未知");
    assert_eq!(ctx.promisors.len(), 1);
    assert_eq!(
        ctx.promisors[0].stable_id(),
        "carol-10003",
        "承诺人 = Carol"
    );
    assert_eq!(ctx.beneficiaries.len(), 1);
    assert_eq!(
        ctx.beneficiaries[0].stable_id(),
        "alice-10001",
        "受益方 = Alice"
    );
    // Bob 绝不成为要求者/承诺人/受益方。
    for list in [&ctx.requesters, &ctx.promisors, &ctx.beneficiaries] {
        assert!(
            !list.iter().any(|p| p.stable_id() == "bob-10002"),
            "Bob 不得成为任何已确认角色"
        );
    }
    for expected in [
        EventRelationKind::SentBy,
        EventRelationKind::RepliesTo,
        EventRelationKind::Mentions,
        EventRelationKind::MemberOfThread,
        EventRelationKind::RequestedBy,
        EventRelationKind::PromisedBy,
        EventRelationKind::Benefits,
    ] {
        assert!(
            ctx.relations.iter().any(|r| r.kind == expected),
            "缺少关系种类 {expected:?}"
        );
    }
    for required in [&e1, &e2, &e3] {
        assert!(
            ctx.source_refs
                .iter()
                .any(|id| id.as_str() == required.as_str()),
            "来源引用必须包含 {}",
            required.as_str()
        );
    }
    personal_secretary::validate_causal_context(&ctx)
        .map_err(|e| format!("因果上下文越界: {e}"))?;
    assert!(
        personal_secretary::check_causal_role_strictness(&ctx).is_empty(),
        "角色语义必须严格合规"
    );

    // 2) 线程根事件：发起人关系来自可重建 VIEW。
    let root_ctx = retriever
        .event_causal_context(&acct_a, &e1)
        .await
        .map_err(|e| format!("root event_causal_context failed: {e}"))?
        .expect("e1 必须可查询");
    assert!(
        root_ctx
            .relations
            .iter()
            .any(|r| r.kind == EventRelationKind::ThreadRootBy
                && r.subject.stable_id() == "alice-10001"),
        "根事件必须带 ThreadRootBy(alice)"
    );

    // 3) 参与者上下文：Carol 的职责与沟通偏好来自已确认人物记忆。
    //    群角色来自会话作用域观察：必须按群会话查询才返回 Admin。
    let conv_a =
        ConversationRef::new(ConversationKind::Group, GROUP_A).expect("valid group A conversation");
    let carol = retriever
        .participant_context(&acct_a, "carol-10003", Some(&conv_a), None)
        .await
        .map_err(|e| format!("participant_context(carol) failed: {e}"))?
        .expect("Carol 必须有参与者上下文");
    assert_eq!(carol.display_name.as_deref(), Some("Carol"));
    assert_eq!(carol.group_role, personal_secretary::GroupRole::Admin);
    let responsibilities: Vec<&str> = carol
        .attributes
        .iter()
        .filter(|a| a.kind == ParticipantAttributeKind::Responsibility)
        .map(|a| a.value.as_str())
        .collect();
    assert_eq!(responsibilities, vec!["负责报告整理"]);
    assert!(
        carol.attributes.iter().any(|a| {
            a.kind == ParticipantAttributeKind::CommunicationPreference
                && a.value == "偏好邮件沟通"
                && a.source_event_ids
                    .iter()
                    .any(|id| id.as_str() == e3.as_str())
        }),
        "沟通偏好必须携带来源 e3"
    );
    personal_secretary::validate_participant_context(&carol)
        .map_err(|e| format!("参与者上下文越界: {e}"))?;
    assert!(
        personal_secretary::check_participant_permission_boundary(&carol).is_empty(),
        "Carol 的上下文不得含权限属性"
    );

    // 4) 参与者上下文：Alice 的 envelope_only 来源记忆不得泄漏。
    //    群名片/群角色按会话观察返回（群 A）。
    let alice = retriever
        .participant_context(&acct_a, "alice-10001", Some(&conv_a), None)
        .await
        .map_err(|e| format!("participant_context(alice) failed: {e}"))?
        .expect("Alice 必须有参与者上下文");
    assert_eq!(alice.display_name.as_deref(), Some("Alice"));
    assert_eq!(alice.group_card.as_deref(), Some("A-名片"));
    assert_eq!(alice.group_role, personal_secretary::GroupRole::Owner);
    assert!(
        !alice
            .attributes
            .iter()
            .any(|a| a.value.contains("来自信封模式")),
        "envelope_only 来源不得支撑人物事实"
    );
    // 未提供会话时群属性必须为未知，绝不跨会话猜测。
    let alice_no_conv = retriever
        .participant_context(&acct_a, "alice-10001", None, None)
        .await
        .map_err(|e| format!("participant_context(alice, no conv) failed: {e}"))?
        .expect("Alice 必须有参与者上下文");
    assert!(
        alice_no_conv.group_card.is_none()
            && alice_no_conv.group_role == personal_secretary::GroupRole::Unknown,
        "无会话时群属性必须为未知"
    );

    // 5) 跨账号隔离：账号 B 复用相同 actor_id 与 message_id。
    // Bob 在账号 B 发过消息（e5），应只有该账号内的最小证据：无显示名、
    // 无群角色、无任何事实/关系 —— 账号 A 的档案、别名、角色与记忆全部不可见。
    let bob_b = retriever
        .participant_context(&acct_b, "bob-10002", None, None)
        .await
        .map_err(|e| format!("participant_context(acct-b) failed: {e}"))?
        .expect("账号 B 内 Bob 有发送事件，必须返回最小上下文");
    assert!(
        bob_b.display_name.is_none() && bob_b.group_card.is_none(),
        "账号 B 不得携带账号 A 的显示信息"
    );
    assert_eq!(bob_b.group_role, personal_secretary::GroupRole::Unknown);
    assert!(
        bob_b.attributes.is_empty(),
        "账号 B 不得携带账号 A 的人物记忆"
    );
    assert!(bob_b.aliases.is_empty(), "账号 B 不得携带账号 A 的别名");
    // 账号 B 只能引用本账号事件（e5），不得出现账号 A 的任何事件 ID。
    assert!(
        bob_b
            .related_event_ids
            .iter()
            .all(|id| id.as_str() == e5.as_str()),
        "账号 B 不得携带账号 A 的事件引用"
    );
    let other = retriever
        .event_causal_context(&acct_b, &e5)
        .await
        .map_err(|e| format!("event_causal_context(acct-b) failed: {e}"))?
        .expect("acct-b 的 e5 必须可查询");
    assert!(
        other.reply_parent.is_none() && other.thread.is_none() && other.participants.is_empty(),
        "acct-b 的事件不得关联 acct-a 的回复/线程/参与者"
    );

    // 5b) 档案历史唯一键回归（P0-1）：Alice 第三次显示名变化必须入库成功，
    //     历史行可无限累积；旧显示名 alias 的来源是建立该显示名的首个事件。
    let e5b1 = ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_A,
            "m-5",
            "alice-10001",
            "改名消息一",
            Vec::new(),
            Some(profile("Alicia", Some("A-名片"), Some("owner"))),
            1_800_000_040,
        ),
    )
    .await?;
    let _e5b2 = ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_A,
            "m-6",
            "alice-10001",
            "改名消息二",
            Vec::new(),
            Some(profile("Alice-新", Some("A-名片"), Some("owner"))),
            1_800_000_050,
        ),
    )
    .await?;
    // 第三次变化后当前显示名 = Alice-新；历史行 ≥ 2（Alice、Alicia），
    // 不再被 UNIQUE(account_id, actor_platform_id, current) 唯一键阻断。
    let history_count = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_participant_profiles \
         WHERE account_id = ? AND actor_platform_id = ? AND current = 0",
        vec![account_id_a.into(), "alice-10001".into()],
    )
    .await;
    assert!(
        history_count >= 2,
        "至少两条历史版本行（Alice、Alicia），实际 {history_count}"
    );
    // alias "Alice" 的来源必须是最早建立该显示名的 e1，而不是触发变化的 m-5/m-6。
    let aliases_json = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(aliases_json AS CHAR) AS value FROM secretary_participant_profiles \
             WHERE account_id = ? AND actor_platform_id = ? AND current = 1",
            vec![account_id_a.into(), "alice-10001".into()],
        ))
        .await
        .map_err(|e| format!("aliases read failed: {e}"))?
        .expect("current profile row must exist")
        .try_get::<String>("", "value")
        .map_err(|e| format!("aliases decode failed: {e}"))?;
    let aliases: Vec<serde_json::Value> = serde_json::from_str(&aliases_json)
        .map_err(|e| format!("aliases json parse failed: {e}"))?;
    let alice_alias = aliases
        .iter()
        .find(|alias| alias.get("alias").and_then(serde_json::Value::as_str) == Some("Alice"))
        .expect("alias Alice 必须存在");
    assert_eq!(
        alice_alias
            .get("source_event_id")
            .and_then(serde_json::Value::as_str),
        Some(e1.as_str()),
        "alias Alice 来源必须是观察到该显示名的 e1，而不是触发变化的 m-5"
    );
    let alicia_alias = aliases
        .iter()
        .find(|alias| alias.get("alias").and_then(serde_json::Value::as_str) == Some("Alicia"))
        .expect("alias Alicia 必须存在");
    assert_eq!(
        alicia_alias
            .get("source_event_id")
            .and_then(serde_json::Value::as_str),
        Some(e5b1.as_str()),
        "alias Alicia 来源必须是观察到该显示名的 m-5，而不是触发变化的 m-6"
    );

    // 5c) 群属性会话作用域（P0-2）：同一 Alice 在群 B 是普通成员，互不覆盖。
    let conv_b =
        ConversationRef::new(ConversationKind::Group, GROUP_B).expect("valid group B conversation");
    let m7_ev = ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_B,
            "m-7",
            "alice-10001",
            "群 B 消息",
            Vec::new(),
            Some(profile("Alice-新", Some("B-名片"), Some("member"))),
            1_800_000_060,
        ),
    )
    .await?;
    let alice_a = retriever
        .participant_context(&acct_a, "alice-10001", Some(&conv_a), None)
        .await
        .map_err(|e| format!("participant_context(alice, A) failed: {e}"))?
        .expect("Alice 在群 A 必须有上下文");
    let alice_b = retriever
        .participant_context(&acct_a, "alice-10001", Some(&conv_b), None)
        .await
        .map_err(|e| format!("participant_context(alice, B) failed: {e}"))?
        .expect("Alice 在群 B 必须有上下文");
    assert_eq!(alice_a.group_card.as_deref(), Some("A-名片"));
    assert_eq!(alice_a.group_role, personal_secretary::GroupRole::Owner);
    assert_eq!(alice_b.group_card.as_deref(), Some("B-名片"));
    assert_eq!(alice_b.group_role, personal_secretary::GroupRole::Member);
    // 群 B 观察绝不污染群 A 查询；账号级显示名仍然一致。
    assert_eq!(
        alice_a.display_name.as_deref(),
        alice_b.display_name.as_deref()
    );

    // 6) 完整 PlannerUseCase 闭环：L0 查询 → Effect → 恰好一条 Response Artifact。
    // CMD-010 防线 A：ActionRun 只能由经过验证的 QQ 开放平台 OwnerCommand
    // 创建（领取/Resume 复验 message_role + actor_kind + active binding），
    // 测试命令事件必须用 OwnerCommand 而非 NapCat 群消息。每段 run 用独立
    // 命令事件（action_runs 唯一键含 command_source_event_id）。
    let cmd_ev = owner_command_with_binding_9_3(
        &db,
        &inbound,
        &acct_a,
        "cmd-acct-9-3",
        "m-cmd-1",
        "查询事件因果",
    )
    .await?;
    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&cmd_ev, "participant-causality-v1");
    let seed = ActionRunSeed {
        account: acct_a.clone(),
        command_source_event_id: cmd_ev.clone(),
        command_text: "查询事件因果".into(),
        conversation_id: "owner-conv".into(),
        occurred_at_unix_secs: 1_800_000_100,
        timezone_offset_secs: 0,
        timezone: "UTC".into(),
        recent_events: vec![RecentEventRef {
            source_event_id: e1.clone(),
            summary: "Owner 命令".into(),
        }],
    };
    action_store
        .ensure_action_run(&run_id, &seed)
        .await
        .map_err(|e| format!("ensure_action_run failed: {e}"))?;
    let use_case = PlannerUseCase::with_clock(
        action_store.clone(),
        Arc::new(CausalQueryPlanner {
            target_event_id: e2.clone(),
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
        Arc::new(SystemClock),
    )
    .with_checkpoint_store_factory(build_mysql_action_checkpoint_store_factory(db.clone()))
    .with_retriever(retriever.clone());
    let report = use_case
        .run_once("test-worker")
        .await
        .map_err(|e| format!("planner run_once failed: {e}"))?
        .expect("planner run must be claimed");
    assert!(!report.suspended, "L0 查询不得挂起");
    assert!(report.completed, "L0 查询必须完成");

    let response_count = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_action_responses WHERE run_id = ?",
        vec![run_id.as_str().into()],
    )
    .await;
    assert_eq!(response_count, 1, "每 run 恰好一条 Response Artifact");
    let receipt_count = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_action_effect_receipts WHERE run_id = ?",
        vec![run_id.as_str().into()],
    )
    .await;
    assert_eq!(receipt_count, 1, "Effect Receipt 恰好一条且幂等");
    let response_json = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(response_json AS CHAR) AS value FROM secretary_action_responses WHERE run_id = ?",
            vec![run_id.as_str().into()],
        ))
        .await
        .map_err(|e| format!("response read failed: {e}"))?
        .expect("response row must exist")
        .try_get::<String>("", "value")
        .map_err(|e| format!("response decode failed: {e}"))?;
    assert!(
        response_json.contains("查询完成"),
        "响应必须是安全中文摘要，而非原始 JSON"
    );

    // 6b) 自然语言人物查询闭环（THR-013-P0-CLOSED-LOOP）：单一复合 L0 动作
    //     GetParticipantContextByName("Carol") → 解析 + 上下文 → 响应含已确认职责。
    //     命令事件必须与 6) 的 e1 不同：action_runs 业务唯一键是
    //     (account_id, command_source_event_id, planner_version)，重复 ensure 会静默跳过。
    // 独立命令事件：action_runs 业务唯一键含 command_source_event_id，
    // 与 6) 共用同一命令会静默跳过创建。
    let cmd_ev2 = owner_command_with_binding_9_3(
        &db,
        &inbound,
        &acct_a,
        "cmd-acct-9-3",
        "m-cmd-2",
        "Carol 负责什么",
    )
    .await?;
    let run_id2 = ActionRunId::for_owner_command(&cmd_ev2, "participant-causality-by-name-v1");
    let seed2 = ActionRunSeed {
        account: acct_a.clone(),
        command_source_event_id: cmd_ev2.clone(),
        command_text: "Carol 负责什么".into(),
        conversation_id: "owner-conv".into(),
        occurred_at_unix_secs: 1_800_000_200,
        timezone_offset_secs: 0,
        timezone: "UTC".into(),
        recent_events: Vec::new(),
    };
    action_store
        .ensure_action_run(&run_id2, &seed2)
        .await
        .map_err(|e| format!("ensure_action_run(by-name) failed: {e}"))?;
    let use_case2 = PlannerUseCase::with_clock(
        action_store,
        Arc::new(NameQueryPlanner {
            name: "Carol".into(),
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
        Arc::new(SystemClock),
    )
    .with_checkpoint_store_factory(build_mysql_action_checkpoint_store_factory(db.clone()))
    .with_retriever(retriever.clone());
    let report2 = use_case2
        .run_once("test-worker")
        .await
        .map_err(|e| format!("planner run_once(by-name) failed: {e}"))?
        .expect("planner run(by-name) must be claimed");
    assert!(!report2.suspended && report2.completed, "复合查询必须完成");
    let name_response_count = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_action_responses WHERE run_id = ?",
        vec![run_id2.as_str().into()],
    )
    .await;
    assert_eq!(name_response_count, 1, "复合查询恰好一条 Response Artifact");
    let name_response = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(response_json AS CHAR) AS value FROM secretary_action_responses WHERE run_id = ?",
            vec![run_id2.as_str().into()],
        ))
        .await
        .map_err(|e| format!("response read failed: {e}"))?
        .expect("response row must exist")
        .try_get::<String>("", "value")
        .map_err(|e| format!("response decode failed: {e}"))?;
    assert!(
        name_response.contains("负责报告整理"),
        "复合查询响应必须携带已确认职责，实际: {name_response}"
    );

    // 7) 隐私失效闭环（P0-3）：删除 e3 正文投影后，Carol 的人物记忆与承诺关系
    //    fail-closed（不再返回）；删除 e1 投影后，Alice 档案来源失效，显示名
    //    不再作为有效事实返回且 expired_or_invalidated 置位。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_message_contents WHERE source_event_id = ?",
        [e3.as_str().into()],
    ))
    .await
    .map_err(|e| format!("projection deletion (e3) failed: {e}"))?;
    let carol_after = retriever
        .participant_context(&acct_a, "carol-10003", Some(&conv_a), None)
        .await
        .map_err(|e| format!("participant_context(carol, after) failed: {e}"))?
        .expect("Carol 必须有参与者上下文");
    assert!(
        carol_after.attributes.is_empty(),
        "正文投影缺失的来源不得支撑人物事实（fail-closed）"
    );
    let e2_after = retriever
        .event_causal_context(&acct_a, &e2)
        .await
        .map_err(|e| format!("event_causal_context(e2, after) failed: {e}"))?
        .expect("e2 必须可查询");
    assert!(
        e2_after.promisors.is_empty() && e2_after.beneficiaries.is_empty(),
        "投影缺失的承诺来源不得支撑 PromisedBy/Benefits（fail-closed）"
    );

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_message_contents WHERE source_event_id = ?",
        [e1.as_str().into()],
    ))
    .await
    .map_err(|e| format!("projection deletion (e1) failed: {e}"))?;
    let alice_after = retriever
        .participant_context(&acct_a, "alice-10001", Some(&conv_a), None)
        .await
        .map_err(|e| format!("participant_context(alice, after) failed: {e}"))?
        .expect("Alice 必须有参与者上下文（事件证据仍在）");
    assert!(
        alice_after.display_name.is_none() && alice_after.aliases.is_empty(),
        "来源投影删除后档案显示名/别名不得作为有效事实返回"
    );
    assert!(
        alice_after.expired_or_invalidated,
        "档案来源全部失效时必须显式标记 expired_or_invalidated"
    );

    // ---- 8) 有界来源淘汰 + 建立事件独立失效（P0-A 反例）----
    // 来源列表满 10 条后，第 11 条建立事件必须保留（淘汰最旧来源），当前值由
    // 它建立并独立失效；若像旧实现那样 push -> truncate(10) 丢弃第 11 条，
    // 删除该事件投影后读取侧只验证旧 10 条，新显示名会错误地继续有效。
    let acct_a_id = scalar_u64(
        &db,
        "SELECT id AS value FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
        vec![MessageSource::NapCat.as_str().into(), ACCT_A.into()],
    )
    .await;
    let conv_a_id = scalar_u64(
        &db,
        "SELECT id AS value FROM secretary_conversations WHERE account_id = ? AND conversation_kind = 'group' AND platform_conversation_id = ?",
        vec![acct_a_id.into(), GROUP_A.into()],
    )
    .await;
    // 造 10 条有效来源事件（bob-10002，群 A）：source_events + message_contents 双表直插。
    let mut seeded: Vec<String> = Vec::new();
    for i in 0..10 {
        let evt = format!("seed-{i:02}");
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"INSERT INTO secretary_source_events
               (source_event_id, account_id, conversation_id, source_channel, platform_event_id,
                event_type, actor_platform_id, actor_kind, message_role,
                occurred_at_unix_secs, received_at)
               VALUES (?, ?, ?, ?, ?, 'message', 'bob-10002', 'external',
                       'external_observation', ?, NOW(6))"#,
            [
                evt.clone().into(),
                acct_a_id.into(),
                conv_a_id.into(),
                MessageSource::NapCat.as_str().into(),
                format!("seed-msg-{i:02}").into(),
                (1_800_000_100i64 + i).into(),
            ],
        ))
        .await
        .map_err(|e| format!("seed source event failed: {e}"))?;
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"INSERT INTO secretary_message_contents
               (source_event_id, normalized_text, segments, mentioned_actor_ids)
               VALUES (?, 'seed', '[]', '[]')"#,
            [evt.clone().into()],
        ))
        .await
        .map_err(|e| format!("seed message contents failed: {e}"))?;
        seeded.push(evt);
    }
    // Bob 当前档案来源覆盖为 10 条有效事件（既有来源被替换，无碍断言）。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"UPDATE secretary_participant_profiles
           SET source_event_ids_json = ?
           WHERE account_id = ? AND platform_identity_kind = 'external'
             AND actor_platform_id = 'bob-10002' AND current = 1"#,
        [
            serde_json::json!(seeded).to_string().into(),
            acct_a_id.into(),
        ],
    ))
    .await
    .map_err(|e| format!("seed profile sources failed: {e}"))?;
    // 第 11 条：改显示名。来源满时淘汰最旧 seed-00，保留本条建立事件。
    let m11 = ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_A,
            "m-11",
            "bob-10002",
            "第 11 条观察",
            Vec::new(),
            Some(profile("Bob-满", None, None)),
            1_800_000_300,
        ),
    )
    .await?;
    let bob_sources_json = scalar_str(
        &db,
        r#"SELECT CAST(source_event_ids_json AS CHAR) AS value
           FROM secretary_participant_profiles
           WHERE account_id = ? AND platform_identity_kind = 'external'
             AND actor_platform_id = 'bob-10002' AND current = 1"#,
        vec![acct_a_id.into()],
    )
    .await;
    let bob_sources: Vec<String> = serde_json::from_str(&bob_sources_json)
        .map_err(|e| format!("bob sources decode failed: {e}"))?;
    assert_eq!(bob_sources.len(), 10, "有界来源必须保持 10 条");
    assert!(
        bob_sources.contains(&m11.as_str().to_owned()),
        "第 11 条建立事件必须保留，不得被 truncate 丢弃"
    );
    assert!(
        !bob_sources.iter().any(|s| s == "seed-00"),
        "最旧来源必须被淘汰以腾出空间"
    );
    // 删除第 11 条投影 → 显示名独立失效（established_by_event_id 指向 m11）。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_message_contents WHERE source_event_id = ?",
        [m11.as_str().into()],
    ))
    .await
    .map_err(|e| format!("projection deletion (m11) failed: {e}"))?;
    let bob_after = retriever
        .participant_context(&acct_a, "bob-10002", Some(&conv_a), None)
        .await
        .map_err(|e| format!("participant_context(bob, after m11) failed: {e}"))?
        .expect("Bob 必须有参与者上下文");
    assert!(
        bob_after.display_name.is_none(),
        "建立事件投影删除后显示名必须失效，实际: {:?}",
        bob_after.display_name
    );

    // ---- 9) 按名查询会话作用域 + 来源有效性（P0-B 反例）----
    // 群名片只在解析出的会话内匹配：Dave 的群 A 名片 "A-名片" 不得在群 B 查询
    // 中命中（旧实现 JOIN 所有会话观察），未提供会话时绝不跨群匹配；
    // 建立事件失效（投影删除）的名片不得参与解析。
    let d1 = ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_A,
            "m-d1",
            "dave-10004",
            "群 A 消息",
            Vec::new(),
            Some(profile("Dave", Some("A-名片"), None)),
            1_800_000_410,
        ),
    )
    .await?;
    ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_B,
            "m-e1",
            "eve-10005",
            "群 B 消息",
            Vec::new(),
            Some(profile("Eve", Some("A-名片"), None)),
            1_800_000_420,
        ),
    )
    .await?;
    let by_name_conv_b = retriever
        .participants_by_display_name(&acct_a, "A-名片", Some(&conv_b), None, 5)
        .await
        .map_err(|e| format!("by-name(conv_b) failed: {e}"))?;
    assert_eq!(
        by_name_conv_b.len(),
        1,
        "群 B 查询必须只命中 Eve 的 B 群名片（Dave 的 A 群名片不得跨群命中），实际: {}",
        by_name_conv_b.len()
    );
    assert_eq!(by_name_conv_b[0].stable_id(), "eve-10005");
    let by_name_conv_a = retriever
        .participants_by_display_name(&acct_a, "A-名片", Some(&conv_a), None, 5)
        .await
        .map_err(|e| format!("by-name(conv_a) failed: {e}"))?;
    assert_eq!(by_name_conv_a.len(), 1, "群 A 查询必须命中 Dave");
    assert_eq!(by_name_conv_a[0].stable_id(), "dave-10004");
    let by_name_none = retriever
        .participants_by_display_name(&acct_a, "A-名片", None, None, 5)
        .await
        .map_err(|e| format!("by-name(None) failed: {e}"))?;
    assert!(by_name_none.is_empty(), "未提供会话时不得跨群匹配群名片");
    // 删除 Dave 群 A 名片建立事件投影 → A-名片不再命中 Dave；Eve 的群 B 名片仍有效。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_message_contents WHERE source_event_id = ?",
        [d1.as_str().into()],
    ))
    .await
    .map_err(|e| format!("projection deletion (d1) failed: {e}"))?;
    let by_name_a_conv_a = retriever
        .participants_by_display_name(&acct_a, "A-名片", Some(&conv_a), None, 5)
        .await
        .map_err(|e| format!("by-name(A-名片, conv_a, invalid) failed: {e}"))?;
    assert!(
        by_name_a_conv_a.is_empty(),
        "建立事件失效的群名片不得参与按名解析"
    );
    let by_name_a_conv_b = retriever
        .participants_by_display_name(&acct_a, "A-名片", Some(&conv_b), None, 5)
        .await
        .map_err(|e| format!("by-name(A-名片, conv_b, still) failed: {e}"))?;
    assert_eq!(by_name_a_conv_b.len(), 1, "Eve 的有效群名片仍应可命中");
    // 删除 m-7（Alice 群 B 名片 "B-名片" 的建立事件）投影 → B-名片不再命中。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_message_contents WHERE source_event_id = ?",
        [m7_ev.as_str().into()],
    ))
    .await
    .map_err(|e| format!("projection deletion (m7) failed: {e}"))?;
    let by_name_b_card = retriever
        .participants_by_display_name(&acct_a, "B-名片", Some(&conv_b), None, 5)
        .await
        .map_err(|e| format!("by-name(B-名片) failed: {e}"))?;
    assert!(
        by_name_b_card.is_empty(),
        "建立事件失效的群名片不得参与按名解析"
    );

    // ---- 10) 身份命名空间隔离（P1-C 反例）----
    // 同账号下同一稳定 ID 以 Owner 身份出现：档案/观察按身份种类隔离并存、
    // 不撞唯一键；participant_context 跨命名空间歧义时 fail-closed 拒绝读取。
    let _owner_ev = ingest(
        inbound.as_ref(),
        InboundMessageEnvelope::new(
            SourceMessageRef::new(MessageSource::NapCat, ACCT_A, "m-owner-1")
                .expect("valid source fixture"),
            conv_a.clone(),
            VerifiedActor::new(VerifiedActorKind::Owner, "alice-10001")
                .expect("valid owner actor fixture"),
            1_800_000_500,
            "Owner 本人消息",
            Vec::new(),
        )
        .expect("valid owner inbound")
        .with_sender_profile(Some(profile("Alice-主", None, None)))
        .expect("valid owner profile"),
    )
    .await?;
    let current_profiles = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_participant_profiles WHERE account_id = ? AND actor_platform_id = ? AND current = 1",
        vec![acct_a_id.into(), "alice-10001".into()],
    )
    .await;
    assert_eq!(
        current_profiles, 2,
        "external 与 owner 档案必须按身份种类隔离并存，不得合并"
    );
    let obs_rows = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_participant_conversation_observations WHERE account_id = ? AND conversation_id = ? AND actor_platform_id = ?",
        vec![acct_a_id.into(), conv_a_id.into(), "alice-10001".into()],
    )
    .await;
    assert_eq!(obs_rows, 2, "群观察同样按身份种类隔离");
    let ambiguous = retriever
        .participant_context(&acct_a, "alice-10001", Some(&conv_a), None)
        .await;
    assert!(
        matches!(&ambiguous, Err(e) if e.to_string().contains("多个身份命名空间")),
        "跨命名空间档案必须 fail-closed 拒绝歧义读取，实际: {ambiguous:?}"
    );
    let owner_candidates = retriever
        .participants_by_display_name(&acct_a, "Alice-主", None, None, 5)
        .await
        .map_err(|e| format!("by-name(owner) failed: {e}"))?;
    assert_eq!(owner_candidates.len(), 1, "Owner 显示名只命中 Owner 档案");
    assert_eq!(
        owner_candidates[0].identity.platform_kind,
        PlatformIdentityKind::Owner,
        "解析出的身份种类必须来自档案键"
    );

    // ---- 11) 同 ID 双 kind 的 by-name → Effect → Response 闭环（P0-1）----
    // 10) 已让 alice-10001 同时存在 external 与 owner 档案；此处以 owner 档案
    // "Alice-主" 走完整 PlannerUseCase：by-name 唯一命中 Owner 三元组 →
    // GetParticipantContextByName Effect 按三元组精确读取（不触发宽松查询的
    // 跨命名空间歧义拒绝）→ Response 成功且携带 Owner 显示名。
    // 若 kind 在 Effect 边界丢失（旧实现只传 actor_id），会退化为宽松查询
    // participant_context 并 fail-closed 报歧义，Response 不可能成功。
    let cmd_ev3 = owner_command_with_binding_9_3(
        &db,
        &inbound,
        &acct_a,
        "cmd-acct-9-3",
        "m-cmd-3",
        "Alice-主 是谁",
    )
    .await?;
    let action_store3 = build_mysql_action_store(db.clone());
    let run_id3 = ActionRunId::for_owner_command(&cmd_ev3, "participant-causality-owner-v1");
    let seed3 = ActionRunSeed {
        account: acct_a.clone(),
        command_source_event_id: cmd_ev3.clone(),
        command_text: "Alice-主 是谁".into(),
        conversation_id: "owner-conv".into(),
        occurred_at_unix_secs: 1_800_000_600,
        timezone_offset_secs: 0,
        timezone: "UTC".into(),
        recent_events: Vec::new(),
    };
    action_store3
        .ensure_action_run(&run_id3, &seed3)
        .await
        .map_err(|e| format!("ensure_action_run(owner by-name) failed: {e}"))?;
    let use_case3 = PlannerUseCase::with_clock(
        action_store3,
        Arc::new(NameQueryPlanner {
            name: "Alice-主".into(),
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
        Arc::new(SystemClock),
    )
    .with_checkpoint_store_factory(build_mysql_action_checkpoint_store_factory(db.clone()))
    .with_retriever(retriever.clone());
    let report3 = use_case3
        .run_once("test-worker")
        .await
        .map_err(|e| format!("planner run_once(owner by-name) failed: {e}"))?
        .expect("planner run(owner by-name) must be claimed");
    assert!(
        !report3.suspended && report3.completed,
        "Owner by-name 复合查询必须完成且不挂起"
    );
    let owner_response_count = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_action_responses WHERE run_id = ?",
        vec![run_id3.as_str().into()],
    )
    .await;
    assert_eq!(
        owner_response_count, 1,
        "Owner by-name 查询恰好一条 Response Artifact"
    );
    let owner_response = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(response_json AS CHAR) AS value FROM secretary_action_responses WHERE run_id = ?",
            vec![run_id3.as_str().into()],
        ))
        .await
        .map_err(|e| format!("owner response read failed: {e}"))?
        .expect("owner response row must exist")
        .try_get::<String>("", "value")
        .map_err(|e| format!("owner response decode failed: {e}"))?;
    assert!(
        owner_response.contains("Alice-主"),
        "Owner by-name 响应必须携带 Owner 档案显示名，实际: {owner_response}"
    );

    // ---- 12) 属性级建立来源门（P0-2 反例）----
    // 建立事件被挤出 10 条有界来源窗口后删除：显示名分支与别名分支都必须按
    // 各自的 established_by_event_id / source_event_id 独立校验，不得因为聚合
    // 来源列表仍然有效而继续命中（旧实现只查 source_event_ids_json）。
    let f1 = ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_A,
            "m-f1",
            "frank-10006",
            "Frank 首条观察",
            Vec::new(),
            Some(profile("Frank-满", None, None)),
            1_800_000_700,
        ),
    )
    .await?;
    for i in 0..10 {
        let msg = format!("m-f{}", i + 2);
        ingest(
            inbound.as_ref(),
            envelope(
                ACCT_A,
                GROUP_A,
                &msg,
                "frank-10006",
                "同值消息",
                Vec::new(),
                Some(profile("Frank-满", None, None)),
                1_800_000_710 + i as i64,
            ),
        )
        .await?;
    }
    // 来源列表满 10 条后 m-f1 已被淘汰，但 established_by_event_id 仍指向 m-f1。
    let frank_sources_json = scalar_str(
        &db,
        r#"SELECT CAST(source_event_ids_json AS CHAR) AS value
           FROM secretary_participant_profiles
           WHERE account_id = ? AND platform_identity_kind = 'external'
             AND actor_platform_id = 'frank-10006' AND current = 1"#,
        vec![acct_a_id.into()],
    )
    .await;
    let frank_sources: Vec<String> = serde_json::from_str(&frank_sources_json)
        .map_err(|e| format!("frank sources decode failed: {e}"))?;
    assert_eq!(frank_sources.len(), 10, "有界来源必须保持 10 条");
    assert!(
        !frank_sources.iter().any(|s| s == f1.as_str()),
        "建立事件 m-f1 必须已被挤出有界来源窗口"
    );
    // 删除建立事件投影 → 显示名分支不得命中。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_message_contents WHERE source_event_id = ?",
        [f1.as_str().into()],
    ))
    .await
    .map_err(|e| format!("projection deletion (f1) failed: {e}"))?;
    let by_name_frank = retriever
        .participants_by_display_name(&acct_a, "Frank-满", Some(&conv_a), None, 5)
        .await
        .map_err(|e| format!("by-name(Frank-满, evicted) failed: {e}"))?;
    assert!(
        by_name_frank.is_empty(),
        "建立事件被挤出窗口且删除后，by-name 不得再命中显示名"
    );
    // 显示名再次变化 → "Frank-满" 进入别名，来源指向已被删除的 m-f1：
    // 别名分支必须按每个别名自己的 source_event_id 独立校验，不得命中。
    ingest(
        inbound.as_ref(),
        envelope(
            ACCT_A,
            GROUP_A,
            "m-f12",
            "frank-10006",
            "改名消息",
            Vec::new(),
            Some(profile("Frank-新", None, None)),
            1_800_000_820,
        ),
    )
    .await?;
    let by_name_frank_alias = retriever
        .participants_by_display_name(&acct_a, "Frank-满", Some(&conv_a), None, 5)
        .await
        .map_err(|e| format!("by-name(Frank-满, alias) failed: {e}"))?;
    assert!(
        by_name_frank_alias.is_empty(),
        "来源已失效的别名不得支撑 by-name 命中"
    );
    let by_name_frank_new = retriever
        .participants_by_display_name(&acct_a, "Frank-新", Some(&conv_a), None, 5)
        .await
        .map_err(|e| format!("by-name(Frank-新) failed: {e}"))?;
    assert_eq!(by_name_frank_new.len(), 1, "有效显示名仍可命中");
    assert_eq!(by_name_frank_new[0].stable_id(), "frank-10006");
    Ok(())
}

/// 线程合并/拆分后，参与者与承诺必须取自有效线程投影（THR-011-P1-EFFECTIVE）。
/// 旧线程（th-old）合并进规范线程（th-new）后：e1 的因果上下文线程应为 th-new，
/// 线程参与者必须包含合并进来的 Alice/Bob，承诺关系按有效线程匹配。
#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn effective_thread_projection_after_merge_mysql() {
    let (db, schema) = isolated_db("_merge").await;
    let outcome = tokio::spawn(merge_scenario(db.clone())).await;
    db.execute_unprepared(&format!("DROP DATABASE IF EXISTS `{schema}`"))
        .await
        .expect("drop effective thread schema");
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(message)) => panic!("effective thread scenario must pass: {message}"),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
}

async fn merge_scenario(db: DatabaseConnection) -> Result<(), String> {
    let acct_m = account("acct-merge");
    let inbound = build_mysql_inbound_event_store(db.clone());
    let group_m = ConversationRef::new(ConversationKind::Group, "g-merge").expect("group fixture");

    let e1 = ingest(
        inbound.as_ref(),
        envelope(
            "acct-merge",
            "g-merge",
            "m-1",
            "alice-10001",
            "旧线程消息一",
            Vec::new(),
            Some(profile("Alice", None, Some("member"))),
            2_100_000_000,
        ),
    )
    .await?;
    let e2 = ingest(
        inbound.as_ref(),
        envelope(
            "acct-merge",
            "g-merge",
            "m-2",
            "bob-10002",
            "旧线程消息二",
            Vec::new(),
            Some(profile("Bob", None, Some("member"))),
            2_100_000_010,
        ),
    )
    .await?;
    let e3 = ingest(
        inbound.as_ref(),
        envelope(
            "acct-merge",
            "g-merge",
            "m-3",
            "carol-10003",
            "规范线程消息",
            Vec::new(),
            Some(profile("Carol", None, Some("admin"))),
            2_100_000_020,
        ),
    )
    .await?;

    let account_id_m = scalar_u64(
        &db,
        "SELECT id AS value FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
        vec!["acct-merge".into()],
    )
    .await;
    // 线程结构：th-old 含 e1/e2，th-new 含 e3。
    for (thread, root, latest, opened) in [
        ("th-old", e1.as_str(), e2.as_str(), 2_100_000_000i64),
        ("th-new", e3.as_str(), e3.as_str(), 2_100_000_020i64),
    ] {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_event_threads \
             (thread_id, account_id, status, root_event_id, latest_event_id, \
              opened_at_unix_secs, latest_occurred_at_unix_secs) \
             VALUES (?, ?, 'open', ?, ?, ?, ?)",
            [
                thread.into(),
                account_id_m.into(),
                root.into(),
                latest.into(),
                opened.into(),
                opened.into(),
            ],
        ))
        .await
        .map_err(|e| format!("thread fixture failed: {e}"))?;
    }
    for (event, thread) in [
        (e1.as_str(), "th-old"),
        (e2.as_str(), "th-old"),
        (e3.as_str(), "th-new"),
    ] {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_thread_events (source_event_id, thread_id) VALUES (?, ?)",
            [event.into(), thread.into()],
        ))
        .await
        .map_err(|e| format!("thread membership fixture failed: {e}"))?;
    }
    // 合并：th-old → th-new（active）。外键要求 mutation proposal 行先存在。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_mutation_proposals \
         (proposal_id, account_id, mutation_kind, impact_json) \
         SELECT ?, id, 'merge', '{}' FROM secretary_accounts \
         WHERE source_channel = 'napcat' AND platform_account_id = ?",
        ["proposal-merge-1".into(), "acct-merge".into()],
    ))
    .await
    .map_err(|e| format!("mutation proposal fixture failed: {e}"))?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_merge_aliases \
         (merged_thread_id, canonical_thread_id, proposal_id, active) \
         VALUES (?, ?, ?, TRUE)",
        ["th-old".into(), "th-new".into(), "proposal-merge-1".into()],
    ))
    .await
    .map_err(|e| format!("merge alias fixture failed: {e}"))?;
    // 承诺记忆：来源 e1（旧线程），合并后必须按有效线程匹配。
    insert_commitment_memory(
        &db,
        "fact-commit-merge",
        &CommitmentMemory {
            promisor: ThreadActorRef {
                platform_identity_kind: None,
                account: acct_m.clone(),
                actor_id: "alice-10001".into(),
            },
            beneficiary: ThreadActorRef {
                platform_identity_kind: None,
                account: acct_m.clone(),
                actor_id: "bob-10002".into(),
            },
            action: "处理合并事项".into(),
            due_at_unix_secs: None,
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        },
        &e1,
    )
    .await?;

    let retriever = Arc::new(RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    ));

    // e1 的有效线程是 th-new（合并后），不再是 th-old。
    let ctx = retriever
        .event_causal_context(&acct_m, &e1)
        .await
        .map_err(|e| format!("event_causal_context(e1) failed: {e}"))?
        .expect("e1 必须可查询");
    let thread = ctx.thread.as_ref().expect("e1 必须属于有效线程");
    assert_eq!(
        thread.thread_id.as_str(),
        "th-new",
        "合并后 e1 必须映射到规范线程 th-new"
    );
    // 参与者来自有效线程投影：包含合并进来的 Alice/Bob 与规范线程的 Carol。
    let actor_ids: Vec<&str> = ctx
        .participants
        .iter()
        .map(|p| p.participant.stable_id())
        .collect();
    for expected in ["alice-10001", "bob-10002", "carol-10003"] {
        assert!(
            actor_ids.contains(&expected),
            "有效线程参与者必须包含 {expected}，实际 {actor_ids:?}"
        );
    }
    // 承诺关系按有效线程匹配（来源 e1 已并入 th-new）。
    assert_eq!(
        ctx.promisors
            .iter()
            .map(|p| p.stable_id())
            .collect::<Vec<_>>(),
        vec!["alice-10001"],
        "承诺人必须来自有效线程匹配的承诺记忆"
    );
    let _ = group_m;
    Ok(())
}

async fn ingest(
    inbound: &dyn personal_secretary::PersonalSecretaryStoreT,
    message: InboundMessageEnvelope,
) -> Result<SourceEventId, String> {
    match inbound
        .insert_message_if_absent(&message)
        .await
        .map_err(|e| format!("ingest failed: {e}"))?
    {
        IngestMessageOutcome::Accepted {
            source_event_id, ..
        } => Ok(source_event_id),
        IngestMessageOutcome::Duplicate { .. } => Err("fixture message must be unique".into()),
    }
}

async fn insert_person_memory(
    db: &DatabaseConnection,
    fact_id: &str,
    payload: &PersonMemory,
    source_event_id: &SourceEventId,
) -> Result<(), String> {
    let fact_json = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_memory_facts \
         (fact_id, account_id, fact_kind, subject_key, fact_json, fact_status, confidence_bps) \
         SELECT ?, id, 'person', ?, ?, 'confirmed', 10000 \
         FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
        [
            fact_id.into(),
            payload.person.actor_id.clone().into(),
            fact_json.into(),
            payload.person.account.account_id.clone().into(),
        ],
    ))
    .await
    .map_err(|e| format!("person memory fixture failed: {e}"))?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_memory_fact_sources (fact_id, source_event_id) VALUES (?, ?)",
        [fact_id.into(), source_event_id.as_str().into()],
    ))
    .await
    .map_err(|e| format!("person memory source fixture failed: {e}"))?;
    Ok(())
}

async fn insert_commitment_memory(
    db: &DatabaseConnection,
    fact_id: &str,
    payload: &CommitmentMemory,
    source_event_id: &SourceEventId,
) -> Result<(), String> {
    let fact_json = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_memory_facts \
         (fact_id, account_id, fact_kind, subject_key, fact_json, fact_status, confidence_bps) \
         SELECT ?, id, 'commitment', ?, ?, 'confirmed', 10000 \
         FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
        [
            fact_id.into(),
            payload.promisor.actor_id.clone().into(),
            fact_json.into(),
            payload.promisor.account.account_id.clone().into(),
        ],
    ))
    .await
    .map_err(|e| format!("commitment memory fixture failed: {e}"))?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_memory_fact_sources (fact_id, source_event_id) VALUES (?, ?)",
        [fact_id.into(), source_event_id.as_str().into()],
    ))
    .await
    .map_err(|e| format!("commitment memory source fixture failed: {e}"))?;
    Ok(())
}
