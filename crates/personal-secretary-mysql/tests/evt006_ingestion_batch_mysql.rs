//! EVT-006 入站微批 + 背压的隔离 MySQL 聚焦测试。
//!
//! 需要 QQBOT_TEST_DATABASE_URL 指向隔离的 MySQL schema（`qqbot_accept_` 前缀）；
//! 默认 #[ignore]。派生 schema 随机命名且测试结束时精确清理。
//!
//! 验证：跨会话/跨账号合成消息在批量事务中正确持久化；重复事件幂等；
//! 事务原子性；账号隔离。

mod common;

use common::{isolated_db, scalar_u64};
use personal_secretary::{
    Clock, ConversationKind, ConversationRef, InboundMessageEnvelope, MessageSource,
    SourceMessageRef, VerifiedActor, VerifiedActorKind,
};
use personal_secretary_mysql::build_mysql_inbound_event_store;
use sea_orm::ConnectionTrait;
use std::sync::Arc;

/// 场景包装：tokio::spawn 确保 panic 后派生 schema 必然在 finally 删除。
#[tokio::test]
#[ignore]
async fn micro_batch_mysql_200_cross_account_messages() {
    let (db, schema) = isolated_db("_evt006s1").await;
    let outcome = tokio::spawn(micro_batch_scenario(db.clone())).await;
    // finally：无论场景成功、失败还是 panic，都先删除派生 schema。
    let cleanup = db
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS `{schema}`"))
        .await;
    if let Err(error) = cleanup {
        eprintln!("schema cleanup skipped (needs DROP privilege): {error}");
    }
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(message)) => panic!("micro batch scenario must pass: {message}"),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
}

async fn micro_batch_scenario(db: sea_orm::DatabaseConnection) -> Result<(), String> {
    let store: Arc<dyn personal_secretary::PersonalSecretaryStoreT> =
        build_mysql_inbound_event_store(db.clone());
    let suffix = uuid::Uuid::new_v4().simple().to_string();

    // ── 1. 构造 200 条跨账号、跨会话的合成消息 ────────────────────────
    let acct_a = &format!("evt006-a-{suffix}");
    let acct_b = &format!("evt006-b-{suffix}");
    let now = personal_secretary::SystemClock.now_unix_secs();
    let mut envelopes: Vec<InboundMessageEnvelope> = Vec::with_capacity(200);

    // 账号 A：group-a 80 条、group-b 70 条
    for i in 0..80 {
        envelopes.push(envelope(
            MessageSource::NapCat,
            acct_a,
            &format!("a-ga-{i}"),
            ConversationKind::Group,
            "group-a",
            "actor-alice",
            now + i,
        ));
    }
    for i in 0..70 {
        envelopes.push(envelope(
            MessageSource::NapCat,
            acct_a,
            &format!("a-gb-{i}"),
            ConversationKind::Group,
            "group-b",
            "actor-bob",
            now + i + 80,
        ));
    }

    // 账号 B：group-b 50 条
    for i in 0..50 {
        envelopes.push(envelope(
            MessageSource::NapCat,
            acct_b,
            &format!("b-gb-{i}"),
            ConversationKind::Group,
            "group-b",
            "actor-carol",
            now + i + 150,
        ));
    }

    assert_eq!(envelopes.len(), 200, "必须恰好 200 条合成消息");

    // ── 2. 批量入库（使用真实的 insert_messages_if_absent 批事务）───
    let results = store
        .insert_messages_if_absent(&envelopes)
        .await
        .map_err(|e| format!("batch insert failed: {e}"))?;

    assert_eq!(results.len(), 200);

    let accepted: Vec<_> = results
        .iter()
        .filter(|o| matches!(o, personal_secretary::IngestMessageOutcome::Accepted { .. }))
        .collect();
    let duplicates: Vec<_> = results
        .iter()
        .filter(|o| {
            matches!(
                o,
                personal_secretary::IngestMessageOutcome::Duplicate { .. }
            )
        })
        .collect();

    assert_eq!(accepted.len(), 200);
    assert_eq!(duplicates.len(), 0);

    // ── 3. 验证 SourceEvent 数量与账号隔离 ──────────────────────────
    let total_events = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_source_events",
        Vec::new(),
    )
    .await;
    assert_eq!(total_events, 200, "必须恰好 200 条 SourceEvent");

    let events_account_a = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_source_events e \
         JOIN secretary_accounts a ON a.id = e.account_id \
         WHERE a.platform_account_id = ?",
        vec![acct_a.to_string().into()],
    )
    .await;
    assert_eq!(events_account_a, 150, "账号 A 150 条（80+70）");

    let events_account_b = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_source_events e \
         JOIN secretary_accounts a ON a.id = e.account_id \
         WHERE a.platform_account_id = ?",
        vec![acct_b.to_string().into()],
    )
    .await;
    assert_eq!(events_account_b, 50, "账号 B 50 条");

    // ── 4. 重复投递：同一批中的重复事件幂等 ─────────────────────────
    // 取前 10 条 + 5 条新消息混合为一个批次，验证 ACID。
    let mut replayed: Vec<InboundMessageEnvelope> = envelopes.iter().take(10).cloned().collect();
    for i in 0..5 {
        replayed.push(envelope(
            MessageSource::NapCat,
            acct_a,
            &format!("a-new-{i}"),
            ConversationKind::Group,
            "group-a",
            "actor-alice",
            now + 300 + i,
        ));
    }
    assert_eq!(replayed.len(), 15);

    let replayed_results = store
        .insert_messages_if_absent(&replayed)
        .await
        .map_err(|e| format!("replay batch failed: {e}"))?;

    let replay_dupes: Vec<_> = replayed_results
        .iter()
        .filter(|o| {
            matches!(
                o,
                personal_secretary::IngestMessageOutcome::Duplicate { .. }
            )
        })
        .collect();
    let replay_accepted: Vec<_> = replayed_results
        .iter()
        .filter(|o| matches!(o, personal_secretary::IngestMessageOutcome::Accepted { .. }))
        .collect();

    assert_eq!(replay_dupes.len(), 10, "前 10 条必须为 Duplicate");
    assert_eq!(replay_accepted.len(), 5, "后 5 条新消息必须为 Accepted");

    // 总事件数仅增加 5。
    let total_after_replay = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_source_events",
        Vec::new(),
    )
    .await;
    assert_eq!(total_after_replay, 205, "重复投递不得新增 SourceEvent");

    // ── 5. 消息内容完整性 ──────────────────────────────────────────
    let contents_count = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_message_contents",
        Vec::new(),
    )
    .await;
    assert_eq!(contents_count, 205, "每条 Accepted 都要有正文投影");

    // ── 6. 会话数 ──────────────────────────────────────────────────
    let conversation_count = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_conversations",
        Vec::new(),
    )
    .await;
    assert_eq!(conversation_count, 3, "acct_a group-a+b, acct_b group-b");

    // ── 7. 数据库中途失败必须整批回滚，恢复后整批重试保持幂等 ──────────
    let rollback_messages = vec![
        envelope(
            MessageSource::NapCat,
            acct_a,
            "evt006-rollback-before",
            ConversationKind::Group,
            "group-a",
            "actor-alice",
            now + 400,
        ),
        envelope(
            MessageSource::NapCat,
            acct_a,
            "evt006-force-rollback",
            ConversationKind::Group,
            "group-a",
            "actor-alice",
            now + 401,
        ),
        envelope(
            MessageSource::NapCat,
            acct_a,
            "evt006-rollback-after",
            ConversationKind::Group,
            "group-a",
            "actor-alice",
            now + 402,
        ),
    ];
    db.execute_unprepared(
        "ALTER TABLE secretary_source_events ADD CONSTRAINT chk_evt006_force_rollback \
         CHECK (platform_event_id <> 'evt006-force-rollback')",
    )
    .await
    .map_err(|error| format!("install rollback constraint failed: {error}"))?;

    let failed = store.insert_messages_if_absent(&rollback_messages).await;
    assert!(
        matches!(
            failed,
            Err(personal_secretary::InboundEventStoreError::Database(_))
        ),
        "数据库约束失败必须返回 Database，不能二分或伪装为 Duplicate"
    );
    let rollback_count = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_source_events \
         WHERE platform_event_id IN (?, ?, ?)",
        vec![
            "evt006-rollback-before".into(),
            "evt006-force-rollback".into(),
            "evt006-rollback-after".into(),
        ],
    )
    .await;
    assert_eq!(rollback_count, 0, "中途失败后不得残留半批 SourceEvent");

    db.execute_unprepared(
        "ALTER TABLE secretary_source_events DROP CHECK chk_evt006_force_rollback",
    )
    .await
    .map_err(|error| format!("remove rollback constraint failed: {error}"))?;
    let recovered = store
        .insert_messages_if_absent(&rollback_messages)
        .await
        .map_err(|error| format!("recovered batch failed: {error}"))?;
    assert!(
        recovered.iter().all(|outcome| matches!(
            outcome,
            personal_secretary::IngestMessageOutcome::Accepted { .. }
        )),
        "数据库恢复后整批必须全部 Accepted"
    );
    let recovered_replay = store
        .insert_messages_if_absent(&rollback_messages)
        .await
        .map_err(|error| format!("recovered replay failed: {error}"))?;
    assert!(
        recovered_replay.iter().all(|outcome| matches!(
            outcome,
            personal_secretary::IngestMessageOutcome::Duplicate { .. }
        )),
        "恢复后的整批重放必须全部 Duplicate"
    );
    let recovered_count = scalar_u64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_source_events \
         WHERE platform_event_id IN (?, ?, ?)",
        vec![
            "evt006-rollback-before".into(),
            "evt006-force-rollback".into(),
            "evt006-rollback-after".into(),
        ],
    )
    .await;
    assert_eq!(recovered_count, 3, "恢复重试与重放只能形成三条业务事实");

    Ok(())
}

/// 辅助：构造最小合法入站信封。
fn envelope(
    source: MessageSource,
    account_id: &str,
    message_id: &str,
    conv_kind: ConversationKind,
    conv_id: &str,
    actor_id: &str,
    occurred_at_unix_secs: i64,
) -> InboundMessageEnvelope {
    InboundMessageEnvelope::new(
        SourceMessageRef::new(source, account_id, message_id).unwrap(),
        ConversationRef::new(conv_kind, conv_id).unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, actor_id).unwrap(),
        occurred_at_unix_secs,
        format!("message text for {message_id}"),
        Vec::new(),
    )
    .unwrap()
}
