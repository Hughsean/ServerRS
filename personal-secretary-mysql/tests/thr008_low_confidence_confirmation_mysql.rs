//! THR-008 low-confidence thread-link confirmation draft against isolated MySQL.
//!
//! Requires QQBOT_TEST_DATABASE_URL pointing to an isolated `qqbot_accept_*` schema.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use personal_secretary::{
    ActionPlannerT, ActionRunId, ActionRunSeed, InMemoryCheckpointStore, PlannerError,
    PlannerInput, PlannerOutput, PlannerUseCase, RetrieverPolicy, RetrieverUseCase,
    SecretaryAction, SecretaryActionProposal, SecretaryAgentState, ThreadLinkReviewUseCase,
    VerifiedActorKind,
};
use personal_secretary_mysql::{
    build_mysql_action_checkpoint_store_factory, build_mysql_action_store,
    build_mysql_inbound_event_store, build_mysql_retriever_store, build_mysql_thread_link_store,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

struct ListPendingLinksPlanner;

#[async_trait]
impl ActionPlannerT for ListPendingLinksPlanner {
    async fn plan(&self, input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        let proposal = SecretaryActionProposal::new(
            SecretaryAction::ListThreadLinkCandidates { limit: 10 },
            "列出待确认的跨会话线程关联候选",
            vec![input.command.source_event_id.clone()],
            None,
        )
        .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?;
        Ok(PlannerOutput::Proposal(proposal))
    }
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn low_confidence_confirmation_is_persisted_without_platform_delivery() {
    let (db, schema) = common::isolated_db("_thr008").await;
    let scenario_db = db.clone();
    let result = tokio::spawn(async move { run_scenario(scenario_db).await }).await;
    common::drop_schema(&db, &schema).await;
    result.expect("THR-008 MySQL scenario must complete");
}

async fn run_scenario(db: DatabaseConnection) {
    let inbound = build_mysql_inbound_event_store(db.clone());
    let managed_id = "thr008-managed";
    let left_source = common::insert_group_message(
        &inbound,
        managed_id,
        "thr008-left-message",
        "thr008-left-group",
        "thr008-left-actor",
        VerifiedActorKind::External,
        1_800_000_001,
        "项目卡片提到上线窗口仍待确认",
    )
    .await;
    let right_source = common::insert_group_message(
        &inbound,
        managed_id,
        "thr008-right-message",
        "thr008-right-group",
        "thr008-right-actor",
        VerifiedActorKind::External,
        1_800_000_002,
        "另一会话出现相同结构化卡片来源",
    )
    .await;
    create_thread(&db, "thr008-thread-a", &left_source, 1_800_000_001).await;
    create_thread(&db, "thr008-thread-b", &right_source, 1_800_000_002).await;
    create_low_confidence_candidate(&db, &left_source, &right_source).await;

    let command_source = common::owner_command_with_binding(
        &db,
        &inbound,
        managed_id,
        "thr008-command-account",
        "thr008-command-message",
        "列出需要我确认的线程关联候选",
        1_800_000_010,
    )
    .await;
    let managed_account = common::account(managed_id);
    let run_id = ActionRunId::generate();
    let action_store = build_mysql_action_store(db.clone());
    action_store
        .ensure_action_run(
            &run_id,
            &ActionRunSeed {
                account: managed_account.clone(),
                command_source_event_id: command_source.clone(),
                command_text: "列出需要我确认的线程关联候选".into(),
                conversation_id: "thr008-owner-control".into(),
                occurred_at_unix_secs: 1_800_000_010,
                timezone_offset_secs: 28_800,
                timezone: "Asia/Shanghai".into(),
                recent_events: Vec::new(),
            },
        )
        .await
        .expect("action run must persist");

    let use_case = PlannerUseCase::new(
        action_store,
        Arc::new(ListPendingLinksPlanner),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_retriever(Arc::new(RetrieverUseCase::new(
        build_mysql_retriever_store(db.clone()),
        RetrieverPolicy::default(),
    )))
    .with_thread_link_review(Arc::new(ThreadLinkReviewUseCase::new(
        build_mysql_thread_link_store(db.clone()),
    )))
    .with_checkpoint_store_factory(build_mysql_action_checkpoint_store_factory(db.clone()));

    let report = use_case
        .run_once("thr008-worker")
        .await
        .expect("planner run must succeed")
        .expect("planner run must be claimable");
    assert!(report.completed);

    let response_json = common::scalar_string(
        &db,
        "SELECT CAST(response_json AS CHAR) AS value FROM secretary_action_responses WHERE run_id = ?",
        vec![run_id.as_str().into()],
    )
    .await;
    let draft: personal_secretary::OwnerResponseDraft =
        serde_json::from_str(&response_json).expect("response draft must deserialize");
    let text = draft
        .segments()
        .iter()
        .map(|segment| segment.text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("低置信度候选"));
    assert!(text.contains("当前有 1 个待确认"));
    assert!(text.contains("请确认接受或拒绝"));
    assert!(text.contains("未确认前不会合并"));
    assert!(text.contains("项目卡片提到上线窗口仍待确认"));
    assert!(draft.source_event_ids().contains(&left_source));
    assert!(draft.source_event_ids().contains(&right_source));
    assert!(!text.contains("投递成功"));
    assert!(!text.contains("已发送"));

    assert_eq!(
        common::scalar_string(
            &db,
            "SELECT status AS value FROM secretary_thread_link_candidates WHERE candidate_id = 'thr008-candidate'",
            Vec::new(),
        )
        .await,
        "proposed"
    );
    assert_eq!(
        common::scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_outbox",
            Vec::new(),
        )
        .await,
        0,
        "local response persistence must not depend on QQ delivery outbox"
    );
}

async fn create_thread(
    db: &DatabaseConnection,
    thread_id: &str,
    source_event_id: &personal_secretary::SourceEventId,
    occurred_at_unix_secs: i64,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_event_threads \
         (thread_id, account_id, status, root_event_id, latest_event_id, \
          opened_at_unix_secs, latest_occurred_at_unix_secs) \
         SELECT ?, account_id, 'open', source_event_id, source_event_id, ?, ? \
         FROM secretary_source_events WHERE source_event_id = ?",
        vec![
            thread_id.into(),
            occurred_at_unix_secs.into(),
            occurred_at_unix_secs.into(),
            source_event_id.as_str().into(),
        ],
    ))
    .await
    .expect("thread must persist");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_events (source_event_id, thread_id) VALUES (?, ?)",
        vec![source_event_id.as_str().into(), thread_id.into()],
    ))
    .await
    .expect("thread membership must persist");
}

async fn create_low_confidence_candidate(
    db: &DatabaseConnection,
    left_source: &personal_secretary::SourceEventId,
    right_source: &personal_secretary::SourceEventId,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_link_candidates \
         (candidate_id, account_id, left_thread_id, right_thread_id, left_conversation_id, \
          right_conversation_id, signal_kind, fingerprint_sha256, status, confidence_bps, \
          reason_code, created_at, updated_at) \
         SELECT 'thr008-accepted', account.id, 'thr008-thread-a', 'thr008-thread-b', \
                left_conversation.id, right_conversation.id, 'exact_file_source_key', ?, \
                'accepted', 9000, 'exact_file_source_key', \
                UTC_TIMESTAMP(6) - INTERVAL 1 SECOND, UTC_TIMESTAMP(6) - INTERVAL 1 SECOND \
         FROM secretary_accounts account \
         JOIN secretary_conversations left_conversation ON left_conversation.account_id = account.id \
         JOIN secretary_conversations right_conversation ON right_conversation.account_id = account.id \
         WHERE account.source_channel = 'napcat' AND account.platform_account_id = 'thr008-managed' \
           AND left_conversation.platform_conversation_id = 'thr008-left-group' \
           AND right_conversation.platform_conversation_id = 'thr008-right-group'",
        vec!["b".repeat(64).into()],
    ))
    .await
    .expect("already reviewed candidate must persist");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_link_candidates \
         (candidate_id, account_id, left_thread_id, right_thread_id, left_conversation_id, \
          right_conversation_id, signal_kind, fingerprint_sha256, status, confidence_bps, reason_code) \
         SELECT 'thr008-candidate', account.id, 'thr008-thread-a', 'thr008-thread-b', \
                left_conversation.id, right_conversation.id, 'exact_rich_content_key', ?, \
                'proposed', 8500, 'exact_rich_content_key' \
         FROM secretary_accounts account \
         JOIN secretary_conversations left_conversation ON left_conversation.account_id = account.id \
         JOIN secretary_conversations right_conversation ON right_conversation.account_id = account.id \
         WHERE account.source_channel = 'napcat' AND account.platform_account_id = 'thr008-managed' \
           AND left_conversation.platform_conversation_id = 'thr008-left-group' \
           AND right_conversation.platform_conversation_id = 'thr008-right-group'",
        vec!["a".repeat(64).into()],
    ))
    .await
    .expect("thread link candidate must persist");
    for source in [left_source, right_source] {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_thread_link_candidate_sources (candidate_id, source_event_id) VALUES ('thr008-candidate', ?)",
            vec![source.as_str().into()],
        ))
        .await
        .expect("candidate source must persist");
    }
}
