//! 项目记忆闭环 + 承诺生命周期闭环（MEM-003/MEM-004）随机隔离 MySQL 测试。
//!
//! 需要 QQBOT_TEST_DATABASE_URL 指向隔离的 MySQL schema（`qqbot_accept_` 前缀）；
//! 默认 #[ignore]。所有 schema 由本文件创建并在测试结束时删除。

use personal_secretary::{
    ActionLeaseToken, ActionRunId, Clock, CommitmentMemory, CommitmentStatus, ConversationKind,
    ConversationRef, FollowUpControlEffectRequest, FollowUpControlStoreError,
    FollowUpControlUseCase, InboundMessageEnvelope, MemoryFact, MemoryFactId, MemoryFactStatus,
    MemoryPayload, MemoryUseCase, MessageSource, PlatformIdentityKind, ProjectMemory,
    RecallCorrelationKey, RecallEvent, RecallEventId, RecallKind, RecallUseCase, RetrieverPolicy,
    RetrieverUseCase, SecretaryAction, SecretaryActionProposal, SourceAccountRef, SourceEventId,
    SourceMessageRef, SystemClock, ThreadActorRef, TombstoneStatus, VerifiedActor,
    VerifiedActorKind,
};
use personal_secretary_mysql::{
    build_mysql_follow_up_control_store, build_mysql_follow_up_store,
    build_mysql_inbound_event_store, build_mysql_memory_store, build_mysql_recall_store,
    build_mysql_retriever_store,
};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use std::sync::Arc;
use uuid::Uuid;

#[path = "../../qqbot-server/database/test_support/qqbot_migrations.rs"]
mod qqbot_migrations;

async fn isolated_db(suffix: &str) -> (DatabaseConnection, String) {
    let base_url = std::env::var("QQBOT_TEST_DATABASE_URL").expect("QQBOT_TEST_DATABASE_URL");
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
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    );
    assert!(
        suffix
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    );
    // MySQL 标识符最多 64 字节。验收脚本生成的基础 schema 名本身可能较长，
    // 因此为固定后缀和随机段预留空间，避免测试只在短名称下通过。
    let random = Uuid::new_v4().simple().to_string();
    let tail = format!("{suffix}-{}", &random[..12]);
    let max_base_len = 64usize
        .checked_sub(tail.len())
        .expect("test schema suffix must fit MySQL identifier limit");
    assert!(max_base_len >= "qqbot_accept_".len());
    let base_prefix = &base_schema[..base_schema.len().min(max_base_len)];
    let schema = format!("{base_prefix}{tail}");
    let base = Database::connect(&base_url).await.expect("connect base");
    base.execute_unprepared(&format!("CREATE DATABASE IF NOT EXISTS `{schema}`"))
        .await
        .expect("create schema");
    drop(base);
    let (prefix, _) = base_url.rsplit_once('/').expect("url parse");
    let url = format!("{prefix}/{schema}");
    let db = Database::connect(url).await.expect("connect derived");
    qqbot_migrations::apply_qqbot_migrations(
        &db,
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../qqbot-server/database/migrations"),
    )
    .await;
    (db, schema)
}

async fn drop_schema(db: &DatabaseConnection, schema: &str) {
    db.execute_unprepared(&format!("DROP DATABASE IF EXISTS `{schema}`"))
        .await
        .ok();
}

async fn scalar_u64(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> u64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .await
        .expect("query")
        .expect("row");
    row.try_get::<u64>("", "value")
        .or_else(|_| row.try_get::<i64>("", "value").map(|v| v as u64))
        .unwrap_or(0)
}

async fn scalar_string(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> String {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .await
        .expect("query")
        .expect("row");
    row.try_get::<String>("", "value").unwrap_or_default()
}

/// 可选字符串查询：无行时返回 None（用于"不应存在"断言）。
async fn optional_scalar_string(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Option<String> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .await
        .expect("query")?;
    let value: Option<String> = row.try_get("", "value").unwrap_or(None);
    value.filter(|v| !v.is_empty())
}

fn account(subject: &str) -> SourceAccountRef {
    SourceAccountRef::new(MessageSource::NapCat, subject).expect("valid account")
}

/// 通过真实入站路径插入群消息：自动建立账号、会话、来源事件和正文投影，
/// 列名与生产 DDL 一致（platform_conversation_id 等）。
#[allow(clippy::too_many_arguments)] // 测试 fixture：字段与入站信封一一对应
async fn insert_group_message(
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    managed_id: &str,
    message_id: &str,
    conversation: &str,
    actor_id: &str,
    actor_kind: VerifiedActorKind,
    occurred_at_unix_secs: i64,
    text: &str,
) -> SourceEventId {
    inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::NapCat, managed_id, message_id).unwrap(),
                ConversationRef::new(ConversationKind::Group, conversation).unwrap(),
                VerifiedActor::new(actor_kind, actor_id).unwrap(),
                occurred_at_unix_secs,
                text,
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .expect("insert message")
        .source_event_id()
        .clone()
}

/// 插入 OwnerCommand（qq_open_platform + owner_control + Owner actor）并建立
/// active OwnerBinding，返回命令事件 ID。
async fn owner_command_with_binding(
    db: &DatabaseConnection,
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    managed_id: &str,
    command_account_id: &str,
    message_id: &str,
    text: &str,
    occurred_at_unix_secs: i64,
) -> SourceEventId {
    let command_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(
                    MessageSource::QqOpenPlatform,
                    command_account_id,
                    message_id,
                )
                .unwrap(),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                occurred_at_unix_secs,
                text,
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .expect("insert owner command")
        .source_event_id()
        .clone();
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_owner_bindings \
             (binding_id, managed_account_id, command_account_id, owner_actor_id, status) \
             SELECT ?, managed.id, command.id, 'owner-openid', 'active' \
             FROM secretary_accounts managed JOIN secretary_accounts command \
             WHERE managed.source_channel = 'napcat' AND managed.platform_account_id = ? \
               AND command.source_channel = 'qq_open_platform' \
               AND command.platform_account_id = ?",
            vec![
                Uuid::new_v4().to_string().into(),
                managed_id.to_owned().into(),
                command_account_id.to_owned().into(),
            ],
        ))
        .await
        .expect("create binding");
    assert_eq!(
        inserted.rows_affected(),
        1,
        "owner binding fixture must create exactly one active OwnerBinding"
    );
    command_event_id
}

/// 插入 running Action Run（含 lease token），返回 lease token。
async fn insert_action_run(
    db: &DatabaseConnection,
    account: &SourceAccountRef,
    run_id: &ActionRunId,
    command_source_event_id: &SourceEventId,
    lease_token: &ActionLeaseToken,
) {
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_action_runs \
             (run_id, account_id, command_source_event_id, command_text, conversation_id, \
              occurred_at_unix_secs, timezone_offset_secs, timezone_name, recent_events_json, \
              status, lease_token, lease_expires_at) \
             SELECT ?, id, ?, '完成跟进', 'owner-conv', ?, 0, 'UTC', JSON_ARRAY(), \
                    'running', ?, UTC_TIMESTAMP(6) + INTERVAL 60 SECOND \
             FROM secretary_accounts \
             WHERE source_channel = ? AND platform_account_id = ?",
            vec![
                run_id.as_str().into(),
                command_source_event_id.as_str().into(),
                SystemClock.now_unix_secs().into(),
                lease_token.as_str().into(),
                account.channel.as_str().into(),
                account.account_id.clone().into(),
            ],
        ))
        .await
        .expect("insert action run");
    assert_eq!(updated.rows_affected(), 1, "action run must be inserted");
}

/// 写入 Pending Commitment Fact（来源为真实入站事件），返回 Fact。
#[allow(clippy::too_many_arguments)] // 测试 fixture：字段与领域结构一一对应
async fn write_pending_commitment(
    memory: &MemoryUseCase,
    account: &SourceAccountRef,
    subject_key: &str,
    source_event_id: &SourceEventId,
    promisor_id: &str,
    beneficiary_id: &str,
    action: &str,
    due_at_unix_secs: Option<i64>,
) -> MemoryFact {
    let fact = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: account.clone(),
        subject_key: subject_key.into(),
        payload: MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                platform_identity_kind: None,
                account: account.clone(),
                actor_id: promisor_id.into(),
            },
            beneficiary: ThreadActorRef {
                platform_identity_kind: None,
                account: account.clone(),
                actor_id: beneficiary_id.into(),
            },
            action: action.into(),
            due_at_unix_secs,
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_500,
        source_event_ids: vec![source_event_id.clone()],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory.remember(&fact).await.expect("remember commitment");
    fact
}

/// 调度器：把到期承诺物化为 FollowUp（source_version=1）。
async fn scan_follow_ups(db: &DatabaseConnection, now_unix_secs: i64) {
    let report = personal_secretary::FollowUpUseCase::new(
        build_mysql_follow_up_store(db.clone()),
        build_mysql_memory_store(db.clone()),
    )
    .scan(now_unix_secs, 86_400, 14_400, 86_400, 100)
    .await
    .expect("scan follow-ups");
    let _ = report;
}

/// 查询承诺 Fact 对应的 FollowUp ID（须恰好一个）。
async fn follow_up_id_for_fact(db: &DatabaseConnection, fact_id: &str) -> String {
    scalar_string(
        db,
        "SELECT item.follow_up_id AS value FROM secretary_follow_up_items item \
         WHERE item.source_memory_fact_id = ?",
        vec![fact_id.into()],
    )
    .await
}

/// 场景 1：项目查询跨账号隔离 + 来源有效性（含撤回失效）。
#[tokio::test]
#[ignore]
async fn project_query_isolation_source_validity() {
    let (db, schema) = isolated_db("_proj").await;
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_a = format!("proj-a-{suffix}");
    let managed_b = format!("proj-b-{suffix}");
    let acct_a = account(&managed_a);
    let acct_b = account(&managed_b);
    let inbound = build_mysql_inbound_event_store(db.clone());

    // 账号 A 的真实来源事件
    let evt_a1 = insert_group_message(
        &inbound,
        &managed_a,
        "proj-msg-1",
        "group-a",
        "alice",
        VerifiedActorKind::External,
        1_800_000_000,
        "项目 alpha 8月上线",
    )
    .await;

    // 写项目记忆到账号 A
    let memory = MemoryUseCase::new(build_mysql_memory_store(db.clone()));
    let fact_a = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: acct_a.clone(),
        subject_key: "project:alpha".into(),
        payload: MemoryPayload::Project(ProjectMemory {
            project_key: "alpha".into(),
            goal: "8月上线".into(),
            member_actor_ids: Vec::new(),
            member_actor_refs: vec![
                personal_secretary::ProjectMemberRef::new(PlatformIdentityKind::External, "alice")
                    .unwrap(),
            ],
            progress: Some("开发中".into()),
            decision_ids: Vec::new(),
            risks: vec!["人力不足".into()],
            blockers: vec!["等待审批".into()],
            artifact_refs: vec!["doc:design".into()],
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 10_000,
        source_event_ids: vec![evt_a1.clone()],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory.remember(&fact_a).await.expect("write project a");

    // 账号 B 也写同 project_key 的项目（跨账号隔离必须按账号区分）
    let evt_b1 = insert_group_message(
        &inbound,
        &managed_b,
        "proj-msg-b1",
        "group-b",
        "bob",
        VerifiedActorKind::External,
        1_800_000_100,
        "项目 alpha 由我负责",
    )
    .await;
    let fact_b = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: acct_b.clone(),
        subject_key: "project:alpha".into(),
        payload: MemoryPayload::Project(ProjectMemory {
            project_key: "alpha".into(),
            goal: "由我负责".into(),
            member_actor_ids: Vec::new(),
            member_actor_refs: vec![
                personal_secretary::ProjectMemberRef::new(PlatformIdentityKind::External, "bob")
                    .unwrap(),
            ],
            progress: None,
            decision_ids: Vec::new(),
            risks: Vec::new(),
            blockers: Vec::new(),
            artifact_refs: Vec::new(),
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 10_000,
        source_event_ids: vec![evt_b1],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory.remember(&fact_b).await.expect("write project b");

    let retriever = RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    );

    // 账号 A 可查到自己的项目
    let projects = retriever.list_projects(&acct_a, 10).await.expect("list a");
    assert!(!projects.is_empty());
    assert_eq!(projects[0].project_key, "alpha");

    let detail = retriever
        .query_project(&acct_a, "alpha")
        .await
        .expect("query a")
        .expect("project exists for a");
    assert_eq!(detail.goal, "8月上线");
    assert_eq!(detail.members.len(), 1);
    assert!(!detail.risks.is_empty());
    // P0-3：来源引用已填充，可回读证据
    assert!(!detail.source_event_ids.is_empty());

    // 账号 B 查到的是自己的项目（同 project_key 隔离）
    let detail_b = retriever
        .query_project(&acct_b, "alpha")
        .await
        .expect("query b")
        .expect("project exists for b");
    assert_eq!(detail_b.goal, "由我负责");

    // 撤回账号 A 的唯一来源 → 项目必须失效（fail-closed，正文派生事实来源不再有效）。
    // 走生产召回路径（RecallUseCase），tombstone 由召回事务写入并 applied。
    let recall = RecallEvent {
        recall_event_id: RecallEventId::new("recall-proj-a1").expect("valid recall id"),
        account: acct_a.clone(),
        kind: RecallKind::Group,
        correlation: RecallCorrelationKey::new(
            acct_a.clone(),
            MessageSource::NapCat,
            ConversationRef::new(ConversationKind::Group, "group-a").unwrap(),
            "proj-msg-1",
        )
        .expect("valid correlation"),
        operator_platform_id: Some("test-operator".into()),
        occurred_at_unix_secs: 1_800_000_500,
    };
    let status = RecallUseCase::new(build_mysql_recall_store(db.clone()))
        .handle_recall(&recall)
        .await
        .expect("recall must apply");
    assert_eq!(status, TombstoneStatus::Applied);
    assert!(
        retriever
            .query_project(&acct_a, "alpha")
            .await
            .expect("query after tombstone")
            .is_none(),
        "revoked source must hide the project"
    );

    drop_schema(&db, &schema).await;
}

/// 场景 2：承诺完成 → 一致性闭环（真实授权链 + 单事务 supersede Pending → Fulfilled）。
#[tokio::test]
#[ignore]
async fn complete_followup_closes_commitment_lifecycle() {
    let (db, schema) = isolated_db("_fuf").await;
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("fuf-managed-{suffix}");
    let acct = account(&managed_id);
    let inbound = build_mysql_inbound_event_store(db.clone());
    let memory = MemoryUseCase::new(build_mysql_memory_store(db.clone()));

    // 1. 真实来源事件 + Pending Commitment Fact
    let commitment_evt = insert_group_message(
        &inbound,
        &managed_id,
        "fuf-msg-1",
        "fuf-group",
        "alice",
        VerifiedActorKind::External,
        1_800_000_000,
        "我会准时发送报价单",
    )
    .await;
    let due = 1_800_604_800;
    let fact = write_pending_commitment(
        &memory,
        &acct,
        "commitment:alice:bob:报价单",
        &commitment_evt,
        "alice",
        "bob",
        "发送报价单",
        Some(due),
    )
    .await;
    let fact_id = fact.fact_id;

    // 2. 调度器：到期承诺 → FollowUp
    scan_follow_ups(&db, due - 1).await;
    let follow_up_id = follow_up_id_for_fact(&db, fact_id.as_str()).await;
    assert!(
        !follow_up_id.is_empty(),
        "commitment must schedule a follow-up"
    );

    // 3. OwnerCommand + active OwnerBinding
    let command_account_id = format!("fuf-command-{suffix}");
    let owner_cmd = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "fuf-cmd-1",
        "完成这条跟进事项",
        due + 60,
    )
    .await;

    // 4. running Action Run + 匹配 lease token
    let run_id = ActionRunId::generate();
    let lease_token = ActionLeaseToken::generate();
    insert_action_run(&db, &acct, &run_id, &owner_cmd, &lease_token).await;

    let control = build_mysql_follow_up_control_store(db.clone());
    let use_case = FollowUpControlUseCase::new(control);

    // 5. 错误版本必须整体拒绝（有效 run/lease/command，新 effect_id）：
    //    版本 CAS 失败 → 事务回滚，FollowUp 仍 scheduled、Fact 仍 Pending。
    let bad_proposal = SecretaryActionProposal {
        proposal_id: "prop-bad-version".to_owned(),
        action: SecretaryAction::CompleteFollowUp {
            follow_up_id: personal_secretary::FollowUpId::new(&follow_up_id).unwrap(),
            expected_source_version: 99,
            reason: "错误版本".into(),
        },
        rationale: "错误版本".into(),
        source_event_ids: vec![owner_cmd.clone()],
        idempotency_key: None,
    };
    let err = use_case
        .apply_effect(&FollowUpControlEffectRequest {
            account: acct.clone(),
            command_source_event_id: owner_cmd.clone(),
            run_id: run_id.clone(),
            lease_token: lease_token.clone(),
            effect_id: "eff-bad-version".to_owned(),
            proposal_id: "prop-bad-version".to_owned(),
            proposal_json: serde_json::to_string(&bad_proposal).unwrap(),
            action: bad_proposal.action,
        })
        .await
        .expect_err("wrong version must be rejected");
    assert!(
        matches!(err, FollowUpControlStoreError::InvalidData(_)),
        "version mismatch must be InvalidData, got {err:?}"
    );
    // 回滚验证：FollowUp 仍 scheduled，旧 Fact 仍 confirmed（Pending），无审计
    let fu_status_after = scalar_string(
        &db,
        "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
        vec![follow_up_id.clone().into()],
    )
    .await;
    assert_eq!(fu_status_after, "scheduled", "rollback must keep scheduled");
    let fact_status_after = scalar_string(
        &db,
        "SELECT fact_status AS value FROM secretary_memory_facts WHERE fact_id = ?",
        vec![fact_id.as_str().into()],
    )
    .await;
    assert_eq!(
        fact_status_after, "confirmed",
        "rollback must keep fact confirmed"
    );
    let audit_count_after = scalar_u64(
        &db,
        "SELECT COUNT(1) AS value FROM secretary_follow_up_owner_controls \
         WHERE follow_up_id = ? AND control_kind = 'complete'",
        vec![follow_up_id.clone().into()],
    )
    .await;
    assert_eq!(audit_count_after, 0, "rollback must not add audit rows");

    // 6. 正确版本 → 成功执行 CompleteFollowUp
    let effect_id = "eff-fulfill";
    let proposal = SecretaryActionProposal {
        proposal_id: "prop-fulfill".to_owned(),
        action: SecretaryAction::CompleteFollowUp {
            follow_up_id: personal_secretary::FollowUpId::new(&follow_up_id).unwrap(),
            expected_source_version: 1,
            reason: "已完成".into(),
        },
        rationale: "完成承诺".into(),
        source_event_ids: vec![owner_cmd.clone()],
        idempotency_key: None,
    };
    let receipt = use_case
        .apply_effect(&FollowUpControlEffectRequest {
            account: acct.clone(),
            command_source_event_id: owner_cmd.clone(),
            run_id: run_id.clone(),
            lease_token: lease_token.clone(),
            effect_id: effect_id.to_owned(),
            proposal_id: "prop-fulfill".to_owned(),
            proposal_json: serde_json::to_string(&proposal).unwrap(),
            action: proposal.action.clone(),
        })
        .await
        .expect("complete must succeed");
    assert!(receipt.result_ref.contains("已完成"));
    assert!(receipt.result_ref.contains("已履行"));

    // 7. 断言：FollowUp completed；旧 Pending superseded；新 Fulfilled 存在
    let fu_status = scalar_string(
        &db,
        "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
        vec![follow_up_id.clone().into()],
    )
    .await;
    assert_eq!(fu_status, "completed");

    let old_status = scalar_string(
        &db,
        "SELECT fact_status AS value FROM secretary_memory_facts WHERE fact_id = ?",
        vec![fact_id.as_str().into()],
    )
    .await;
    assert_eq!(old_status, "superseded");

    let mem_store = build_mysql_memory_store(db.clone());
    let active = mem_store.list_active(&acct, 50).await.expect("list");
    let fulfilled: Vec<_> = active
        .iter()
        .filter(|f| {
            matches!(&f.payload, MemoryPayload::Commitment(c) if c.status == CommitmentStatus::Fulfilled)
        })
        .collect();
    assert_eq!(fulfilled.len(), 1);
    // 完成来源事件已写入新 Fact 的来源集合（completion_source_event_id 回读）
    let fulfilled_fact = &fulfilled[0];
    assert!(
        fulfilled_fact.source_event_ids.contains(&owner_cmd),
        "completion event must be part of fulfilled fact sources"
    );

    // 8. 审计存在
    let audit_count = scalar_u64(
        &db,
        "SELECT COUNT(1) AS value FROM secretary_follow_up_owner_controls \
         WHERE follow_up_id = ? AND control_kind = 'complete'",
        vec![follow_up_id.clone().into()],
    )
    .await;
    assert_eq!(audit_count, 1);

    // 9. 幂等重放：相同 effect_id 返回同一回执，不重复写
    let receipt2 = use_case
        .apply_effect(&FollowUpControlEffectRequest {
            account: acct.clone(),
            command_source_event_id: owner_cmd.clone(),
            run_id: run_id.clone(),
            lease_token: lease_token.clone(),
            effect_id: effect_id.to_owned(),
            proposal_id: "prop-fulfill".to_owned(),
            proposal_json: serde_json::to_string(&proposal).unwrap(),
            action: proposal.action.clone(),
        })
        .await
        .expect("idempotent replay");
    assert_eq!(
        receipt2.result_ref, receipt.result_ref,
        "CAS replay returns same receipt"
    );

    drop_schema(&db, &schema).await;
}

/// 场景 3：无截止时间不生成 FollowUp；批量完成原子闭合多个承诺。
#[tokio::test]
#[ignore]
async fn no_due_no_followup_and_batch_complete_closes_all() {
    let (db, schema) = isolated_db("_bat").await;
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("bat-managed-{suffix}");
    let acct = account(&managed_id);
    let inbound = build_mysql_inbound_event_store(db.clone());
    let memory = MemoryUseCase::new(build_mysql_memory_store(db.clone()));

    // 1. 无截止时间的承诺 → 不生成 FollowUp
    let no_due_evt = insert_group_message(
        &inbound,
        &managed_id,
        "bat-msg-0",
        "bat-group",
        "alice",
        VerifiedActorKind::External,
        1_800_000_000,
        "我之后会处理这件事",
    )
    .await;
    let no_due_fact = write_pending_commitment(
        &memory,
        &acct,
        "commitment:alice:bob:无期限",
        &no_due_evt,
        "alice",
        "bob",
        "之后处理",
        None,
    )
    .await;
    scan_follow_ups(&db, 1_800_604_800).await;
    let no_due_follow_up = optional_scalar_string(
        &db,
        "SELECT item.follow_up_id AS value FROM secretary_follow_up_items item \
         WHERE item.source_memory_fact_id = ?",
        vec![no_due_fact.fact_id.as_str().into()],
    )
    .await;
    assert!(
        no_due_follow_up.is_none(),
        "commitment without due must not schedule a follow-up"
    );

    // 2. 两个有期限承诺 → 各自生成 FollowUp
    let due = 1_800_604_800;
    let evt_a = insert_group_message(
        &inbound,
        &managed_id,
        "bat-msg-a",
        "bat-group",
        "alice",
        VerifiedActorKind::External,
        1_800_000_100,
        "我会交付 A",
    )
    .await;
    let fact_a = write_pending_commitment(
        &memory,
        &acct,
        "commitment:alice:bob:A",
        &evt_a,
        "alice",
        "bob",
        "交付 A",
        Some(due),
    )
    .await;
    let evt_b = insert_group_message(
        &inbound,
        &managed_id,
        "bat-msg-b",
        "bat-group",
        "alice",
        VerifiedActorKind::External,
        1_800_000_200,
        "我会交付 B",
    )
    .await;
    let fact_b = write_pending_commitment(
        &memory,
        &acct,
        "commitment:alice:bob:B",
        &evt_b,
        "alice",
        "bob",
        "交付 B",
        Some(due),
    )
    .await;
    scan_follow_ups(&db, due - 1).await;
    let fu_a = follow_up_id_for_fact(&db, fact_a.fact_id.as_str()).await;
    let fu_b = follow_up_id_for_fact(&db, fact_b.fact_id.as_str()).await;
    assert!(!fu_a.is_empty() && !fu_b.is_empty(), "both must schedule");

    // 3. OwnerCommand + binding + Action Run
    let command_account_id = format!("bat-command-{suffix}");
    let owner_cmd = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "bat-cmd-1",
        "完成这两条跟进",
        due + 60,
    )
    .await;
    let run_id = ActionRunId::generate();
    let lease_token = ActionLeaseToken::generate();
    insert_action_run(&db, &acct, &run_id, &owner_cmd, &lease_token).await;

    // 4. 批量完成（原子 all-or-nothing）
    let control = build_mysql_follow_up_control_store(db.clone());
    let use_case = FollowUpControlUseCase::new(control);
    let good_targets = vec![
        personal_secretary::FollowUpControlTarget {
            follow_up_id: personal_secretary::FollowUpId::new(&fu_a).unwrap(),
            expected_source_version: 1,
        },
        personal_secretary::FollowUpControlTarget {
            follow_up_id: personal_secretary::FollowUpId::new(&fu_b).unwrap(),
            expected_source_version: 1,
        },
    ];

    // 4a. 错误批量（B 版本错误）→ 整体回滚：A/B 均仍 scheduled，Fact 均仍 Pending
    let bad_proposal = SecretaryActionProposal {
        proposal_id: "prop-batch-bad".to_owned(),
        action: SecretaryAction::CompleteFollowUps {
            targets: vec![
                personal_secretary::FollowUpControlTarget {
                    follow_up_id: personal_secretary::FollowUpId::new(&fu_a).unwrap(),
                    expected_source_version: 1,
                },
                personal_secretary::FollowUpControlTarget {
                    follow_up_id: personal_secretary::FollowUpId::new(&fu_b).unwrap(),
                    expected_source_version: 99,
                },
            ],
            reason: "错误版本".into(),
        },
        rationale: "批量错误".into(),
        source_event_ids: vec![owner_cmd.clone()],
        idempotency_key: None,
    };
    let err = use_case
        .apply_effect(&FollowUpControlEffectRequest {
            account: acct.clone(),
            command_source_event_id: owner_cmd.clone(),
            run_id: run_id.clone(),
            lease_token: lease_token.clone(),
            effect_id: "eff-batch-bad".to_owned(),
            proposal_id: "prop-batch-bad".to_owned(),
            proposal_json: serde_json::to_string(&bad_proposal).unwrap(),
            action: bad_proposal.action,
        })
        .await
        .expect_err("wrong version must be rejected");
    assert!(
        matches!(err, FollowUpControlStoreError::InvalidData(_)),
        "batch version mismatch must be InvalidData, got {err:?}"
    );
    for fu in [&fu_a, &fu_b] {
        let status = scalar_string(
            &db,
            "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
            vec![fu.clone().into()],
        )
        .await;
        assert_eq!(
            status, "scheduled",
            "all-or-nothing: follow-up {fu} must stay scheduled"
        );
    }
    for fact in [&fact_a, &fact_b] {
        let status = scalar_string(
            &db,
            "SELECT fact_status AS value FROM secretary_memory_facts WHERE fact_id = ?",
            vec![fact.fact_id.as_str().into()],
        )
        .await;
        assert_eq!(
            status,
            "confirmed",
            "all-or-nothing: fact {} must stay confirmed",
            fact.fact_id.as_str()
        );
    }

    // 4b. 正确批量 → 两个承诺都闭合
    let proposal = SecretaryActionProposal {
        proposal_id: "prop-batch".to_owned(),
        action: SecretaryAction::CompleteFollowUps {
            targets: good_targets,
            reason: "全部完成".into(),
        },
        rationale: "批量完成".into(),
        source_event_ids: vec![owner_cmd.clone()],
        idempotency_key: None,
    };
    let receipt = use_case
        .apply_effect(&FollowUpControlEffectRequest {
            account: acct.clone(),
            command_source_event_id: owner_cmd.clone(),
            run_id: run_id.clone(),
            lease_token: lease_token.clone(),
            effect_id: "eff-batch".to_owned(),
            proposal_id: "prop-batch".to_owned(),
            proposal_json: serde_json::to_string(&proposal).unwrap(),
            action: proposal.action.clone(),
        })
        .await
        .expect("batch complete must succeed");
    assert!(receipt.result_ref.contains("2"));

    // 5. 断言：两个 FollowUp completed；两个旧 Fact superseded；两个新 Fulfilled
    for fu in [&fu_a, &fu_b] {
        let status = scalar_string(
            &db,
            "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
            vec![fu.clone().into()],
        )
        .await;
        assert_eq!(status, "completed", "follow-up {fu} must be completed");
    }
    for fact in [&fact_a, &fact_b] {
        let status = scalar_string(
            &db,
            "SELECT fact_status AS value FROM secretary_memory_facts WHERE fact_id = ?",
            vec![fact.fact_id.as_str().into()],
        )
        .await;
        assert_eq!(
            status,
            "superseded",
            "fact {} must be superseded",
            fact.fact_id.as_str()
        );
    }
    let mem_store = build_mysql_memory_store(db.clone());
    let active = mem_store.list_active(&acct, 50).await.expect("list");
    let fulfilled_count = active
        .iter()
        .filter(|f| {
            matches!(&f.payload, MemoryPayload::Commitment(c) if c.status == CommitmentStatus::Fulfilled)
        })
        .count();
    assert_eq!(fulfilled_count, 2);

    // 6. 两条 complete 审计都存在
    for fu in [&fu_a, &fu_b] {
        let audit = scalar_u64(
            &db,
            "SELECT COUNT(1) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id = ? AND control_kind = 'complete'",
            vec![fu.clone().into()],
        )
        .await;
        assert_eq!(audit, 1, "audit must exist for {fu}");
    }

    drop_schema(&db, &schema).await;
}
