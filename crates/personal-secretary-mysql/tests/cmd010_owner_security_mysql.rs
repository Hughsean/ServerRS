//! CMD-010 Owner 越权防线 + 跨会话指代歧义防线的隔离 MySQL 测试。
//!
//! 需要 QQBOT_TEST_DATABASE_URL 指向隔离的 MySQL schema（`qqbot_accept_` 前缀）；
//! 默认 #[ignore]。schema 由 tests/common 共享夹具创建并在测试结束时删除。
//!
//! 场景 1：NapCat 群管理员伪 Owner 指令不创建 ActionRun；授权四元组
//! （managed + command + owner actor + identity kind）任一不匹配时
//! 领取/Resume/Effect 均拒绝且无副作用；Suspend 后撤销 OwnerBinding，
//! Resume 整体拒绝。
//! 场景 2：同账号两个群存在同名参与者；无作用域的非显式指代不跨群选人
//! （返回空候选/澄清），显式会话作用域精确解析，跨账号数据不可见。

mod common;

use common::{
    account, drop_schema, insert_action_run, insert_group_message, isolated_db,
    owner_command_with_binding, scalar_u64,
};
use personal_secretary::{
    ActionLeaseToken, ActionRunId, AgendaApplyRequest, AgendaMutation, Clock, ConversationKind,
    ConversationRef, ReferenceContext, RetrieverPolicy, RetrieverUseCase, SourceEventId,
    SuspendedRunClaim, SystemClock, VerifiedActorKind,
};
use personal_secretary_mysql::{
    build_mysql_action_store, build_mysql_agenda_store, build_mysql_inbound_event_store,
    build_mysql_retriever_store,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};

/// 把命令事件伪装成非 Owner 身份（模拟权威 SourceEvent 被错误写入或
/// 回补来源 kind 异常；message_role 保持不变，只有 actor_kind 非 owner）。
async fn fake_command_actor_kind(db: &DatabaseConnection, event_id: &SourceEventId, kind: &str) {
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "UPDATE secretary_source_events SET actor_kind = ? WHERE source_event_id = ?",
            vec![kind.into(), event_id.as_str().into()],
        ))
        .await
        .expect("update actor_kind");
    assert_eq!(updated.rows_affected(), 1, "command event must exist");
}

/// 直接插入一条历史 pending Run，用于证明领取层会拦截升级前遗留的未授权数据。
/// 新生产入口必须走 ensure_action_run，并已在场景首段断言伪命令无法创建 Run。
async fn insert_legacy_pending_run(
    db: &DatabaseConnection,
    account: &personal_secretary::SourceAccountRef,
    run_id: &ActionRunId,
    command_source_event_id: &SourceEventId,
    command_text: &str,
    occurred_at_unix_secs: i64,
) {
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "INSERT INTO secretary_action_runs \
             (run_id, account_id, command_source_event_id, command_text, conversation_id, \
              occurred_at_unix_secs, timezone_offset_secs, timezone_name, recent_events_json, status) \
             SELECT ?, id, ?, ?, 'owner-conv', ?, 0, 'UTC', JSON_ARRAY(), 'pending' \
             FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
            vec![
                run_id.as_str().into(),
                command_source_event_id.as_str().into(),
                command_text.to_owned().into(),
                occurred_at_unix_secs.into(),
                account.channel.as_str().into(),
                account.account_id.clone().into(),
            ],
        ))
        .await
        .expect("insert legacy pending action run");
    assert_eq!(inserted.rows_affected(), 1, "legacy run fixture inserted");
}

/// 撤销（非删除）托管账号的全部 active OwnerBinding（吊销语义）。
async fn revoke_all_bindings(db: &DatabaseConnection, managed_id: &str) {
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "UPDATE secretary_owner_bindings b \
             JOIN secretary_accounts a ON a.id = b.managed_account_id \
             SET b.status = 'revoked' \
             WHERE a.source_channel = 'napcat' AND a.platform_account_id = ?",
            vec![managed_id.into()],
        ))
        .await
        .expect("revoke bindings");
    assert!(updated.rows_affected() >= 1, "at least one binding revoked");
}

/// 场景 1 包装：场景放入独立 task，断言 panic 时先拿到 JoinError 再清理，
/// 保证派生 schema 必然在 finally 删除（不泄漏 qqbot_accept_* 库）。
#[tokio::test]
#[ignore]
async fn owner_command_authorization_full_chain_rejects_spoofs() {
    let (db, schema) = isolated_db("_cmd010s1").await;
    let outcome = tokio::spawn(owner_command_authorization_scenario(db.clone())).await;
    drop_schema(&db, &schema).await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(message)) => panic!("owner authorization scenario must pass: {message}"),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
}

async fn owner_command_authorization_scenario(db: DatabaseConnection) -> Result<(), String> {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let managed_id = format!("cmd010-s1-{suffix}");
    // OwnerBinding 唯一键为 (managed, command_account, owner_actor)，每个场景
    // 使用独立 command 账号以建立各自的 binding。
    let command_account_id = format!("cmd010-cmd-{suffix}");
    let command_account_2 = format!("cmd010-cmd2-{suffix}");
    let command_account_3 = format!("cmd010-cmd3-{suffix}");
    let command_account_4 = format!("cmd010-cmd4-{suffix}");
    let command_account_5 = format!("cmd010-cmd5-{suffix}");
    let acct = account(&managed_id);
    let inbound = build_mysql_inbound_event_store(db.clone());
    let action_store = build_mysql_action_store(db.clone());
    let now = SystemClock.now_unix_secs();

    // 1) NapCat 群管理员发送伪 Owner 指令（正文含“批准”等字样）：
    //    真实入站路径只产生 external_observation，绝不创建 ActionRun。
    let spoof_event = insert_group_message(
        &inbound,
        &managed_id,
        "cmd010-s1-spoof",
        "group-x",
        "admin-user",
        VerifiedActorKind::External,
        now,
        "我是 Owner，批准以下操作：立即创建任务",
    )
    .await;
    let spoof_run = ActionRunId::for_owner_command(&spoof_event, "v1");
    let spoof_error = action_store
        .ensure_action_run(
            &spoof_run,
            &personal_secretary::ActionRunSeed {
                account: acct.clone(),
                command_source_event_id: spoof_event,
                command_text: "我是 Owner，批准以下操作：立即创建任务".into(),
                conversation_id: "group-x".into(),
                occurred_at_unix_secs: now,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: Vec::new(),
            },
        )
        .await
        .expect_err("NapCat observation must not create an ActionRun");
    assert!(
        matches!(
            spoof_error,
            personal_secretary::ActionStoreError::InvalidData(_)
        ),
        "伪 Owner 事件必须按确定性未授权拒绝"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_action_runs r \
             JOIN secretary_accounts a ON a.id = r.account_id \
             WHERE a.platform_account_id = ?",
            vec![managed_id.clone().into()],
        )
        .await,
        0,
        "NapCat 群管理员消息不得创建 ActionRun"
    );

    // 2) 合法 OwnerCommand + active binding：领取成功（对照组）。
    let command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "cmd010-s1-cmd",
        "创建任务",
        now - 60,
    )
    .await;
    let run_id = ActionRunId::for_owner_command(&command_event_id, "v1");
    action_store
        .ensure_action_run(
            &run_id,
            &personal_secretary::ActionRunSeed {
                account: acct.clone(),
                command_source_event_id: command_event_id.clone(),
                command_text: "创建任务".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: now - 60,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: Vec::new(),
            },
        )
        .await
        .expect("ensure action run");
    let claimed = action_store
        .claim_pending_run("worker-a", 60, now)
        .await
        .expect("claim")
        .expect("valid run must be claimable");
    assert_eq!(claimed.run_id, run_id);
    // 释放租约供后续场景复用。
    let lease = ActionLeaseToken::generate();
    let _ = lease;

    // 3) 四元组不匹配（一）：命令事件 actor_kind 被改为 external →
    //    新 pending run 不可领取（identity kind 防线）。
    let spoofed_event = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_2,
        "cmd010-s1-cmd2",
        "创建任务二号",
        now - 30,
    )
    .await;
    fake_command_actor_kind(&db, &spoofed_event, "external").await;
    let spoofed_run = ActionRunId::for_owner_command(&spoofed_event, "v1");
    insert_legacy_pending_run(
        &db,
        &acct,
        &spoofed_run,
        &spoofed_event,
        "创建任务二号",
        now - 30,
    )
    .await;
    assert!(
        action_store
            .claim_pending_run("worker-b", 60, now)
            .await
            .expect("claim")
            .is_none(),
        "actor_kind 非 owner 的命令不得被领取"
    );

    // 4) 四元组不匹配（二）：owner actor 不匹配（binding owner 与命令
    //    actor 不同）→ 不领取。伪造第二条 binding 与命令事件不匹配。
    let mismatched_event = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_3,
        "cmd010-s1-cmd3",
        "创建任务三号",
        now - 20,
    )
    .await;
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::MySql,
            "UPDATE secretary_owner_bindings b \
             JOIN secretary_accounts a ON a.id = b.managed_account_id \
             JOIN secretary_accounts c ON c.id = b.command_account_id \
             SET b.owner_actor_id = 'someone-else' \
             WHERE a.source_channel = 'napcat' AND a.platform_account_id = ? \
               AND c.source_channel = 'qq_open_platform' AND c.platform_account_id = ?",
            vec![managed_id.clone().into(), command_account_3.clone().into()],
        ))
        .await
        .expect("mismatch binding owner");
    assert_eq!(updated.rows_affected(), 1, "binding owner updated");
    let mismatched_run = ActionRunId::for_owner_command(&mismatched_event, "v1");
    insert_legacy_pending_run(
        &db,
        &acct,
        &mismatched_run,
        &mismatched_event,
        "创建任务三号",
        now - 20,
    )
    .await;
    assert!(
        action_store
            .claim_pending_run("worker-c", 60, now)
            .await
            .expect("claim")
            .is_none(),
        "owner actor 不匹配的命令不得被领取"
    );

    // 5) Suspend 后撤销 OwnerBinding：Resume 整体拒绝（claim_suspended_run
    //    不领取），业务表、审计与成功 Receipt 均无副作用。
    let resume_event = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_4,
        "cmd010-s1-cmd4",
        "创建任务四号",
        now - 10,
    )
    .await;
    let resume_run = ActionRunId::for_owner_command(&resume_event, "v1");
    let resume_lease = ActionLeaseToken::generate();
    insert_action_run(&db, &acct, &resume_run, &resume_event, &resume_lease).await;
    let checkpoint_json = serde_json::json!({
        "checkpoint_id": "ckpt-cmd010-1",
        "proposal_id": "prop-cmd010-1",
        "reason": "approval",
    })
    .to_string();
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::MySql,
        "UPDATE secretary_action_runs SET status = 'suspended', last_checkpoint_json = ?, \
         lease_token = NULL, lease_expires_at = NULL \
         WHERE run_id = ?",
        vec![checkpoint_json.into(), resume_run.as_str().into()],
    ))
    .await
    .expect("suspend run");
    // 场景 6 的命令与 binding 必须在撤销之前建立：撤销动作吊销本托管账号的
    // 全部 active binding（含 cmd5），随后 Agenda Effect 才会以"已撤销"身份被拒。
    let revoked_event = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_5,
        "cmd010-s1-cmd5",
        "创建任务五号",
        now - 5,
    )
    .await;
    let revoked_run = ActionRunId::for_owner_command(&revoked_event, "v1");
    let revoked_lease = ActionLeaseToken::generate();
    insert_action_run(&db, &acct, &revoked_run, &revoked_event, &revoked_lease).await;
    revoke_all_bindings(&db, &managed_id).await;
    let resumed = action_store
        .claim_suspended_run(&SuspendedRunClaim {
            run_id: resume_run.clone(),
            checkpoint_id: "ckpt-cmd010-1".into(),
            proposal_id: "prop-cmd010-1".into(),
            command_source_event_id: resume_event.clone(),
            worker_id: "owner-resume".into(),
            lease_secs: 60,
            now_unix_secs: now,
        })
        .await
        .expect("claim suspended");
    assert!(
        resumed.is_none(),
        "binding 撤销后 Resume 必须拒绝（不领取）"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_action_effect_receipts",
            Vec::new(),
        )
        .await,
        0,
        "不得写成功 Effect Receipt"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_agenda_items",
            Vec::new(),
        )
        .await,
        0,
        "不得修改业务状态"
    );

    // 6) 撤销后写类 Effect 兜底拒绝：Agenda 事务内复验 OwnerCommand，
    //    失败时整笔回滚（不写审计/Receipt）。命令、run 与 binding 已在撤销前建立。
    let agenda = build_mysql_agenda_store(db.clone());
    let agenda_request = AgendaApplyRequest {
        account: acct.clone(),
        command_source_event_id: revoked_event.clone(),
        run_id: revoked_run.as_str().to_owned(),
        effect_id: "cmd010-s1-effect".into(),
        proposal_id: "prop-cmd010-2".into(),
        proposal_json: "{}".into(),
        lease_token: revoked_lease.as_str().to_owned(),
        idempotency_key: "cmd010-s1-idem".into(),
        mutation: AgendaMutation::Create {
            kind: personal_secretary::AgendaItemKind::Task,
            title: "被拒绝的任务".into(),
            scheduled_at_unix_secs: Some(now + 3600),
            timezone: "UTC".into(),
        },
    };
    let agenda_error = agenda
        .apply(&agenda_request, now)
        .await
        .expect_err("binding 撤销后 Agenda Effect 必须拒绝");
    assert!(
        agenda_error.to_string().contains("not authorized"),
        "拒绝原因必须是 OwnerCommand 授权失败，got: {agenda_error}"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_agenda_items",
            Vec::new(),
        )
        .await,
        0,
        "Effect 拒绝不得修改业务状态"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_agenda_mutation_audit",
            Vec::new(),
        )
        .await,
        0,
        "Effect 拒绝不得写审计"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_action_effect_receipts",
            Vec::new(),
        )
        .await,
        0,
        "Effect 拒绝不得写成功 Receipt"
    );

    Ok(())
}

/// 场景 2 包装：同样先清理派生 schema 再判定场景结果，panic 时
/// schema 必然在 finally 删除（与场景 1 同一模式）。
#[tokio::test]
#[ignore]
async fn reference_resolution_scoped_no_cross_group_or_cross_account() {
    let (db, schema) = isolated_db("_cmd010s2").await;
    let outcome = tokio::spawn(reference_resolution_scenario(db.clone())).await;
    drop_schema(&db, &schema).await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(message)) => panic!("reference resolution scenario must pass: {message}"),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
}

async fn reference_resolution_scenario(db: DatabaseConnection) -> Result<(), String> {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let managed_a = format!("cmd010-a-{suffix}");
    let managed_b = format!("cmd010-b-{suffix}");
    let acct_a = account(&managed_a);
    let acct_b = account(&managed_b);
    let inbound = build_mysql_inbound_event_store(db.clone());
    let retriever = RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    );
    let now = SystemClock.now_unix_secs();

    // 账号 A：group-a 与 group-b 都有 alice（同昵称、同稳定 ID、同 kind），
    // 以及 group-b 的 bob；账号 B：group-b 也有 alice（跨账号同名）。
    insert_group_message(
        &inbound,
        &managed_a,
        "cmd010-a-a1",
        "group-a",
        "alice",
        VerifiedActorKind::External,
        now - 3600,
        "项目 alpha 报价完成",
    )
    .await;
    insert_group_message(
        &inbound,
        &managed_a,
        "cmd010-a-b1",
        "group-b",
        "alice",
        VerifiedActorKind::External,
        now - 1800,
        "项目 beta 讨论中",
    )
    .await;
    insert_group_message(
        &inbound,
        &managed_a,
        "cmd010-a-b2",
        "group-b",
        "bob",
        VerifiedActorKind::External,
        now - 900,
        "报价单已发出",
    )
    .await;
    insert_group_message(
        &inbound,
        &managed_b,
        "cmd010-b-b1",
        "group-b",
        "alice",
        VerifiedActorKind::External,
        now - 300,
        "跨账号报价",
    )
    .await;

    // 1) 无作用域的非显式指代：不跨群选人——不返回账号级模糊候选
    //    （用例层据此生成 OpenReference/澄清，不猜测“最新一个”）。
    let scoped_ctx = |account: &personal_secretary::SourceAccountRef,
                      conversation: Option<ConversationRef>| {
        ReferenceContext {
            account: account.clone(),
            current_conversation: conversation,
            current_thread_id: None,
            recent_events: Vec::new(),
            now_unix_secs: now,
            timezone: "Asia/Shanghai".into(),
        }
    };
    let unscoped = retriever
        .resolve_reference(&acct_a, "alice", &scoped_ctx(&acct_a, None))
        .await
        .expect("resolve unscoped");
    assert!(
        unscoped.ambiguous,
        "无作用域时指代必须歧义（不跨群猜唯一），got: {}",
        unscoped.evidence
    );
    assert!(unscoped.resolved_actor_id.is_none(), "不得猜测最新一个");

    // 2) 显式会话作用域（group-a）：精确解析到 group-a 的 alice。
    let in_group_a = retriever
        .resolve_reference(
            &acct_a,
            "alice",
            &scoped_ctx(
                &acct_a,
                Some(ConversationRef::new(ConversationKind::Group, "group-a").unwrap()),
            ),
        )
        .await
        .expect("resolve group-a");
    assert!(
        !in_group_a.ambiguous,
        "显式作用域 + 唯一候选应精确解析，got: {}",
        in_group_a.evidence
    );
    assert_eq!(in_group_a.resolved_actor_id.as_deref(), Some("alice"));

    // 3) 显式会话作用域（group-b）：alice + bob 同时命中 alice 表达式？
    //    alice 是唯一匹配 alice 的，但同一群内候选唯一 → 精确解析（alice）。
    let in_group_b = retriever
        .resolve_reference(
            &acct_a,
            "alice",
            &scoped_ctx(
                &acct_a,
                Some(ConversationRef::new(ConversationKind::Group, "group-b").unwrap()),
            ),
        )
        .await
        .expect("resolve group-b");
    assert!(
        !in_group_b.ambiguous,
        "group-b 作用域内 alice 唯一，应精确解析"
    );
    assert_eq!(in_group_b.resolved_actor_id.as_deref(), Some("alice"));

    // 4) 跨账号隔离：账号 B 的同名 alice 绝不进入账号 A 的候选
    //    （group-b 作用域内账号 A 只有 alice + bob；账号 B 的 alice 不可见）。
    let for_b = retriever
        .resolve_reference(
            &acct_b,
            "alice",
            &scoped_ctx(
                &acct_b,
                Some(ConversationRef::new(ConversationKind::Group, "group-b").unwrap()),
            ),
        )
        .await
        .expect("resolve account B");
    assert!(
        !for_b.ambiguous && for_b.resolved_actor_id.as_deref() == Some("alice"),
        "账号 B 作用域解析自己账号内的 alice"
    );
    // 账号 B 在 group-a 无任何事件：即使账号 A 的 alice 匹配表达式，
    // 账号 B 视角的 group-a 候选必须为空（歧义信号）。
    let b_in_group_a = retriever
        .resolve_reference(
            &acct_b,
            "alice",
            &scoped_ctx(
                &acct_b,
                Some(ConversationRef::new(ConversationKind::Group, "group-a").unwrap()),
            ),
        )
        .await
        .expect("resolve account B group-a");
    assert!(
        b_in_group_a.ambiguous,
        "跨账号候选永远不可见：账号 B 在 group-a 无候选"
    );
    assert!(
        b_in_group_a.resolved_actor_id.is_none(),
        "不得解析到账号 A 的 alice"
    );

    Ok(())
}
