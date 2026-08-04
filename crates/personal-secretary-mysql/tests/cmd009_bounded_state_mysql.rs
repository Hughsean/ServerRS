//! CMD-009 跨阶段有界状态 + 长期事件检索排序 + 冲突驱动回读的隔离 MySQL 测试。
//!
//! 需要 QQBOT_TEST_DATABASE_URL 指向隔离的 MySQL schema（`qqbot_accept_` 前缀）；
//! 默认 #[ignore]。schema 由 tests/common 共享夹具创建并在测试结束时删除。
//!
//! 场景 1：SearchRecentEvents 未指定 since 时可检索 24 小时以前的长期事件；
//! 显式时间窗 / conversation / actor 硬过滤；相关性（前缀 > 包含）→ 时间 DESC
//! → source_event_id DESC 的确定性排序；LIKE 特殊字符转义；跨账号与撤回隔离。
//! 场景 2：记忆候选批准冲突是确定性业务结果——结构化冲突回执、候选保持
//! proposed、不覆盖现行事实、不重复写批准审计；L0 回读（evidence）在来源
//! 撤回后 fail-closed（来源集合收缩）。

mod common;

use common::{
    account, drop_schema, insert_action_run, insert_group_message, isolated_db,
    owner_command_with_binding, scalar_string, scalar_u64,
};
use personal_secretary::{
    ActionLeaseToken, ActionRunId, Clock, ConversationKind, ConversationRef,
    MemoryCandidateConflictResultV1, MemoryConflictReasonCode, MemoryFact, MemoryFactId,
    MemoryFactStatus, MemoryPayload, MemoryUseCase, MessageSource, ProjectMemory,
    RecallCorrelationKey, RecallEvent, RecallEventId, RecallKind, RecallUseCase, RetrieverPolicy,
    RetrieverUseCase, SecretaryAction, SecretaryActionProposal, SecretaryActionReceipt,
    SourceEventId, SystemClock, TombstoneStatus, VerifiedActorKind,
};
use personal_secretary_mysql::{
    build_mysql_inbound_event_store, build_mysql_memory_candidate_control_store,
    build_mysql_memory_store, build_mysql_recall_store, build_mysql_retriever_store,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};

/// 执行以 `SELECT ..., id, ... FROM secretary_accounts` 结尾的账号作用域 fixture
/// INSERT（追加账号过滤条件并断言恰好插入一行）。
async fn insert_account_scoped(
    db: &DatabaseConnection,
    managed_id: &str,
    sql: &str,
    mut values: Vec<sea_orm::Value>,
) {
    let sql = format!(
        "{sql} FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?"
    );
    values.push(managed_id.into());
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            &sql,
            values,
        ))
        .await
        .expect("fixture insert");
    assert_eq!(
        updated.rows_affected(),
        1,
        "fixture must insert exactly one row"
    );
}

fn project_payload_json(goal: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "project",
        "data": {
            "project_key": "alpha",
            "goal": goal,
            "member_actor_ids": [],
            "member_actor_refs": [],
            "progress": null,
            "decision_ids": [],
            "risks": [],
            "blockers": [],
            "artifact_refs": []
        }
    })
}

/// 断言恰好两元素且顺序符合 source_event_id DESC（随机 ID 决胜，无法硬编码谁先）。
fn assert_id_desc_pair(actual: &[String], a: &SourceEventId, b: &SourceEventId, ctx: &str) {
    let pair = [a.as_str().to_owned(), b.as_str().to_owned()];
    assert!(
        actual.len() == 2 && pair.contains(&actual[0]) && pair.contains(&actual[1]),
        "{ctx}，got {actual:?}，expected pair {pair:?}"
    );
    assert!(
        actual[0] > actual[1],
        "{ctx}：同时间戳按 source_event_id DESC"
    );
}

/// 有界检索断言辅助：构造 EventQuery 并返回 source_event_id 列表。
async fn search_ids(
    retriever: &RetrieverUseCase,
    account: &personal_secretary::SourceAccountRef,
    query_text: &str,
    since_unix_secs: Option<i64>,
    conversation: Option<&str>,
    actor_id: Option<&str>,
) -> Vec<String> {
    let results = retriever
        .search_events(
            &personal_secretary::EventQuery {
                account: account.clone(),
                conversation: conversation
                    .map(|id| ConversationRef::new(ConversationKind::Group, id).unwrap()),
                actor_id: actor_id.map(str::to_owned),
                thread_id: None,
                since_unix_secs,
                until_unix_secs: None,
                query_text: Some(query_text.into()),
                limit: 20,
            },
            false,
        )
        .await
        .expect("search");
    results
        .iter()
        .map(|r| r.source_event_id.as_str().to_owned())
        .collect()
}

/// 场景 1：长期事件检索窗口 + 确定性排序 + 跨账号/撤回隔离。
#[tokio::test]
#[ignore]
async fn search_events_long_window_deterministic_order_isolation() {
    let (db, schema) = isolated_db("_cmd009s1").await;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let managed_a = format!("cmd009-a-{suffix}");
    let managed_b = format!("cmd009-b-{suffix}");
    let acct_a = account(&managed_a);
    let acct_b = account(&managed_b);
    let inbound = build_mysql_inbound_event_store(db.clone());
    let now = SystemClock.now_unix_secs();
    let day = 86_400i64;

    // 账号 A：group-x（bob）与 group-y（alice）；a1/a2 是 3 天前的长期事件，
    // a3/a4 是 2 小时内的近期事件（同时间戳，用于 ID 决胜排序）。
    let evt_a1 = insert_group_message(
        &inbound,
        &managed_a,
        "cmd009-a1",
        "group-x",
        "bob",
        VerifiedActorKind::External,
        now - 3 * day,
        "项目报价单 X100% 确定",
    )
    .await;
    let evt_a2 = insert_group_message(
        &inbound,
        &managed_a,
        "cmd009-a2",
        "group-x",
        "bob",
        VerifiedActorKind::External,
        now - 3 * day - 100,
        "关于报价单的讨论记录",
    )
    .await;
    let evt_a3 = insert_group_message(
        &inbound,
        &managed_a,
        "cmd009-a3",
        "group-y",
        "alice",
        VerifiedActorKind::External,
        now - 2 * 3600,
        "报价单已经更新",
    )
    .await;
    let evt_a4 = insert_group_message(
        &inbound,
        &managed_a,
        "cmd009-a4",
        "group-y",
        "alice",
        VerifiedActorKind::External,
        now - 2 * 3600,
        "报价单来了",
    )
    .await;
    let evt_b1 = insert_group_message(
        &inbound,
        &managed_b,
        "cmd009-b1",
        "group-x",
        "bob",
        VerifiedActorKind::External,
        now - 3600,
        "报价单隔离测试",
    )
    .await;
    let retriever = RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    );

    // 1) 未指定 since → 24 小时以前的长期事件可检索（原先被 24h 窗口排除）。
    let all = search_ids(&retriever, &acct_a, "报价单", None, None, None).await;
    assert!(
        all.contains(&evt_a1.as_str().to_owned()),
        "3 天前事件必须可检索"
    );
    // 2) 确定性排序：相关性（前缀 > 包含）优先，同级按 occurred_at DESC，
    //    同时间戳按 source_event_id DESC（a3/a4 谁先由随机 ID 决胜，只能断言对）。
    assert_id_desc_pair(&all[..2], &evt_a4, &evt_a3, "前缀匹配事件必须占据前两位");
    assert_eq!(all[2], evt_a1.as_str());
    assert_eq!(all[3], evt_a2.as_str());

    // 3) 显式时间窗硬过滤：since=1 天内 → 只返回近期事件（同级 ID DESC）。
    let recent = search_ids(&retriever, &acct_a, "报价单", Some(now - day), None, None).await;
    assert_id_desc_pair(&recent, &evt_a4, &evt_a3, "时间窗过滤后只返回近期事件");

    // 4) conversation / actor 硬过滤。
    let in_group_x = search_ids(&retriever, &acct_a, "报价单", None, Some("group-x"), None).await;
    assert_eq!(
        in_group_x,
        vec![evt_a1.as_str().to_owned(), evt_a2.as_str().to_owned()]
    );
    let by_alice = search_ids(&retriever, &acct_a, "报价单", None, None, Some("alice")).await;
    assert_id_desc_pair(
        &by_alice,
        &evt_a4,
        &evt_a3,
        "actor 过滤后只返回 alice 的事件",
    );

    // 5) LIKE 转义：查询字面 "100%" 只命中 a1（% 不按通配展开）。
    let escaped = search_ids(&retriever, &acct_a, "100%", None, None, None).await;
    assert_eq!(escaped, vec![evt_a1.as_str().to_owned()]);

    // 6) 跨账号隔离：账号 B 查不到账号 A 的任何事件。
    let for_b = search_ids(&retriever, &acct_b, "报价单", None, None, None).await;
    assert_eq!(for_b, vec![evt_b1.as_str().to_owned()]);

    // 7) 撤回隔离：召回 a3 后从检索结果中消失。
    let recall = RecallEvent {
        recall_event_id: RecallEventId::new("recall-cmd009-a3").expect("valid recall id"),
        account: acct_a.clone(),
        kind: RecallKind::Group,
        correlation: RecallCorrelationKey::new(
            acct_a.clone(),
            MessageSource::NapCat,
            ConversationRef::new(ConversationKind::Group, "group-y").unwrap(),
            "cmd009-a3",
        )
        .expect("valid correlation"),
        operator_platform_id: Some("test-operator".into()),
        occurred_at_unix_secs: now,
    };
    assert_eq!(
        RecallUseCase::new(build_mysql_recall_store(db.clone()))
            .handle_recall(&recall)
            .await
            .expect("recall must apply"),
        TombstoneStatus::Applied
    );
    let after_recall = search_ids(&retriever, &acct_a, "报价单", None, None, None).await;
    assert!(
        !after_recall.contains(&evt_a3.as_str().to_owned()),
        "撤回事件必须从检索结果中消失"
    );

    drop_schema(&db, &schema).await;
}

/// 场景 2：记忆候选批准冲突 → 结构化回执 + 不覆盖事实 + 不重复批准审计 + 回读 fail-closed。
#[tokio::test]
#[ignore]
async fn candidate_approval_conflict_structured_receipt_no_overwrite() {
    let (db, schema) = isolated_db("_cmd009s2").await;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let managed_id = format!("cmd009-s2-{suffix}");
    let acct = account(&managed_id);
    let inbound = build_mysql_inbound_event_store(db.clone());
    let memory = MemoryUseCase::new(build_mysql_memory_store(db.clone()));
    let now = SystemClock.now_unix_secs();

    // 现行事实（同 subject_key、不同 goal）→ 冲突目标。
    let existing_src = insert_group_message(
        &inbound,
        &managed_id,
        "cmd009-s2-src0",
        "group-y",
        "alice",
        VerifiedActorKind::External,
        now - 2 * 86_400,
        "项目 alpha 8月上线",
    )
    .await;
    let existing_fact = MemoryFact {
        fact_id: MemoryFactId::new("existing-project-fact-000001").expect("valid fact id"),
        account: acct.clone(),
        subject_key: "project:alpha".into(),
        payload: MemoryPayload::Project(ProjectMemory {
            project_key: "alpha".into(),
            goal: "8月上线".into(),
            member_actor_ids: Vec::new(),
            member_actor_refs: Vec::new(),
            progress: None,
            decision_ids: Vec::new(),
            risks: Vec::new(),
            blockers: Vec::new(),
            artifact_refs: Vec::new(),
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 10_000,
        source_event_ids: vec![existing_src.clone()],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory
        .remember(&existing_fact)
        .await
        .expect("remember existing fact");

    // 候选（proposed v1）：goal 不同 → 与现行事实冲突。
    let candidate_src = insert_group_message(
        &inbound,
        &managed_id,
        "cmd009-s2-src1",
        "group-y",
        "alice",
        VerifiedActorKind::External,
        now - 3600,
        "项目 alpha 9月上线",
    )
    .await;
    let candidate_id = "candidate-conflict-0000001";
    insert_account_scoped(
        &db,
        &managed_id,
        "INSERT INTO secretary_memory_candidates \
         (candidate_id, account_id, candidate_kind, subject_key, payload_json, \
          candidate_status, candidate_version, extractor_version, deterministic_fingerprint) \
         SELECT ?, id, 'project', 'project:alpha', ?, 'proposed', 1, 'test', ?",
        vec![
            candidate_id.into(),
            project_payload_json("9月上线").to_string().into(),
            "a".repeat(64).into(),
        ],
    )
    .await;
    insert_account_scoped(
        &db,
        &managed_id,
        "INSERT INTO secretary_memory_candidate_sources \
         (candidate_id, source_event_id, account_id, actor_platform_id, \
          content_trust_level, occurred_at_unix_secs) \
         SELECT ?, ?, id, 'alice', 'normal', ?",
        vec![
            candidate_id.into(),
            candidate_src.as_str().into(),
            now.into(),
        ],
    )
    .await;

    // 授权链 fixture：OwnerCommand + 绑定 + running Action Run。
    let command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &format!("cmd009-cmd-{suffix}"),
        "cmd009-s2-cmd",
        "批准这个记忆候选",
        now - 60,
    )
    .await;
    let run_id = ActionRunId::generate();
    let lease_token = ActionLeaseToken::generate();
    insert_action_run(&db, &acct, &run_id, &command_event_id, &lease_token).await;

    // 批准 Effect：冲突是确定性业务结果，不走 supersede 也不推进候选状态机。
    let action = SecretaryAction::ApproveMemoryCandidate {
        candidate_id: personal_secretary::MemoryCandidateId::new(candidate_id).unwrap(),
        expected_candidate_version: 1,
        reason: "批准候选".into(),
    };
    let proposal = SecretaryActionProposal::new(
        action.clone(),
        "批准记忆候选",
        vec![candidate_src.clone()],
        Some("cmd009-s2-idem-1".into()),
    )
    .expect("proposal");
    let use_case = personal_secretary::MemoryCandidateControlUseCase::new(
        build_mysql_memory_candidate_control_store(db.clone()),
    );
    let request = personal_secretary::MemoryCandidateControlEffectRequest {
        account: acct.clone(),
        command_source_event_id: command_event_id.clone(),
        run_id: run_id.clone(),
        lease_token: lease_token.clone(),
        effect_id: "cmd009-s2-effect-1".into(),
        proposal_id: proposal.proposal_id.clone(),
        proposal_json: serde_json::to_string(&proposal).expect("proposal json"),
        action,
    };
    let receipt: SecretaryActionReceipt = use_case
        .apply_effect(&request)
        .await
        .expect("apply approve conflict");

    // 结构化冲突回执：版本 1、候选/事实 ID 匹配、原因码为内容冲突。
    let conflict: MemoryCandidateConflictResultV1 =
        serde_json::from_str(&receipt.result_ref).expect("conflict result must be structured JSON");
    assert_eq!(conflict.version, 1);
    assert_eq!(conflict.candidate_id.as_str(), candidate_id);
    assert_eq!(conflict.fact_id.as_str(), existing_fact.fact_id.as_str());
    assert_eq!(
        conflict.reason_code,
        MemoryConflictReasonCode::ActiveFactPayloadDiffers
    );
    assert!(
        !conflict.summary.is_empty(),
        "conflict summary must be bounded Chinese text"
    );

    // 候选保持 proposed 且版本不变；现行事实未被覆盖（同 subject 事实仍只有一条）。
    assert_eq!(
        scalar_string(
            &db,
            "SELECT candidate_status AS value FROM secretary_memory_candidates WHERE candidate_id = ?",
            vec![candidate_id.into()],
        )
        .await,
        "proposed"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT candidate_version AS value FROM secretary_memory_candidates WHERE candidate_id = ?",
            vec![candidate_id.into()],
        )
        .await,
        1
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_memory_facts WHERE account_id IN \
             (SELECT id FROM secretary_accounts WHERE platform_account_id = ?) \
             AND subject_key = 'project:alpha'",
            vec![managed_id.clone().into()],
        )
        .await,
        1,
        "冲突不得新建或覆盖事实"
    );

    // 重放同一 Effect：幂等返回同一回执，审计数量不变（只写一条 approve_conflict）。
    let replay = use_case
        .apply_effect(&request)
        .await
        .expect("replay must be idempotent");
    assert_eq!(
        replay.result_ref, receipt.result_ref,
        "重放必须返回同一回执"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_memory_candidate_controls WHERE candidate_id = ?",
            vec![candidate_id.into()],
        )
        .await,
        1,
        "重放不得重复写批准审计"
    );

    // L0 回读（冲突驱动回读的输入）：现行事实与有效来源可完整回读。
    let view = memory
        .evidence(&existing_fact.fact_id, 800)
        .await
        .expect("evidence re-read")
        .expect("fact must still exist");
    assert_eq!(
        view.fact.payload, existing_fact.payload,
        "事实内容不得被覆盖"
    );
    assert_eq!(
        view.sources
            .iter()
            .map(|s| s.source_event_id.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec![existing_src.as_str().to_owned()],
        "回读必须返回现行事实的全部有效来源"
    );

    // 撤回现行事实的唯一来源 → 回读来源集合收缩（fail-closed 的输入信号）。
    let recall = RecallEvent {
        recall_event_id: RecallEventId::new("recall-cmd009-s2").expect("valid recall id"),
        account: acct.clone(),
        kind: RecallKind::Group,
        correlation: RecallCorrelationKey::new(
            acct.clone(),
            MessageSource::NapCat,
            ConversationRef::new(ConversationKind::Group, "group-y").unwrap(),
            "cmd009-s2-src0",
        )
        .expect("valid correlation"),
        operator_platform_id: Some("test-operator".into()),
        occurred_at_unix_secs: now,
    };
    assert_eq!(
        RecallUseCase::new(build_mysql_recall_store(db.clone()))
            .handle_recall(&recall)
            .await
            .expect("recall must apply"),
        TombstoneStatus::Applied
    );
    let view_after_recall = memory
        .evidence(&existing_fact.fact_id, 800)
        .await
        .expect("evidence after recall")
        .expect("fact row still exists");
    assert!(
        view_after_recall.sources.is_empty(),
        "撤回来源必须从回读来源集合中消失"
    );

    drop_schema(&db, &schema).await;
}
