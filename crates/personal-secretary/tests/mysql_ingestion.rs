use agent_core::AgentState;
use agent_core::graph::{
    GraphDefinition, GraphExecutionResult, GraphId, GraphPolicy, GraphRuntime, NodeId, RunBudget,
    TransitionRule,
};
use personal_secretary::{
    AgendaApplyRequest, AgendaItemKind, AgendaMutation, AgendaUseCase, BackfillAnchor,
    BackfillBudget, BackfillCursor, BackfillEvidence, BackfillGapUseCase, BackfillLease,
    BackfillOutcome, BackfillScopeStatus, Clock, CommitmentMemory, CommitmentStatus,
    ConnectionEndReason, ConservativeThreadSemanticExtractor, ContentSegment, ContentTrustLevel,
    ConversationKind, ConversationMemoryModeInput, ConversationRef, DeterministicThreadPlanner,
    DeterministicThreadPolicy, DirectoryEvidence, DirectorySnapshot, DirectorySnapshotId,
    DirectorySourceApi, DirectoryStatus, EvaluationCommitResult, EventThreadId,
    HistoryBackfillSourceT, HistoryCompleteness, InboundMessageEnvelope, IngestMessageOutcome,
    IngestionGapReason, IngestionGapStatus, LegacyNotificationReconciliationConfig,
    MemoryDeleteInput, MemoryFact, MemoryFactId, MemoryFactStatus, MemoryPayload, MemoryUseCase,
    MessageSource, NotificationFailureKind, NotificationPolicyEvaluator, NotificationPolicyUseCase,
    OwnerNotificationContent, PersonMemory, ProjectMemory, ScopeProgress, SourceAccountRef,
    SourceMessageRef, SystemClock, ThreadActorRef, ThreadLinkCandidateId, ThreadLinkReviewAction,
    ThreadLinkReviewUseCase, ThreadLinkUseCase, ThreadMutationApprovalNode, ThreadMutationDecision,
    ThreadMutationDecisionNode, ThreadMutationEffect, ThreadMutationEffectExecutor,
    ThreadMutationImpact, ThreadMutationKind, ThreadMutationProposalId, ThreadMutationResumeInput,
    ThreadMutationRevertInput, ThreadMutationRevertUseCase, ThreadMutationStoreT,
    ThreadMutationUseCase, ThreadProjectionUseCase, ThreadSemanticUseCase, VerifiedActor,
    VerifiedActorKind, build_mysql_agenda_store, build_mysql_backfill_store,
    build_mysql_directory_store, build_mysql_follow_up_store, build_mysql_inbound_event_store,
    build_mysql_memory_store, build_mysql_notification_policy_store, build_mysql_thread_link_store,
    build_mysql_thread_mutation_checkpoint_store, build_mysql_thread_mutation_store,
    build_mysql_thread_projection_store, build_mysql_thread_semantic_store,
};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[path = "../../../apps/qqbot-server/database/test_support/qqbot_migrations.rs"]
mod qqbot_migrations;

fn thread_mutation_runtime(
    db: sea_orm::DatabaseConnection,
    store: Arc<dyn ThreadMutationStoreT>,
) -> GraphRuntime<personal_secretary::ThreadMutationAgentState> {
    let approval_id = NodeId::try_from("thread_mutation_approval").unwrap();
    let decision_id = NodeId::try_from("thread_mutation_decision").unwrap();
    let mut definition = GraphDefinition::new(GraphId::try_from("thread-mutation").unwrap());
    definition
        .add_node(Arc::new(ThreadMutationApprovalNode::new().unwrap()))
        .unwrap();
    definition
        .add_node(Arc::new(
            ThreadMutationDecisionNode::new(store.clone()).unwrap(),
        ))
        .unwrap();
    definition.set_entry(approval_id.clone());
    definition
        .set_transition(approval_id, TransitionRule::Goto(decision_id.clone()))
        .unwrap();
    definition
        .set_transition(decision_id, TransitionRule::End)
        .unwrap();
    let graph = definition
        .compile(GraphPolicy::new(NonZeroU32::new(4).unwrap()))
        .unwrap();
    GraphRuntime::with_effect_executor(graph, Arc::new(ThreadMutationEffectExecutor::new(store)))
        .with_checkpoint_store(build_mysql_thread_mutation_checkpoint_store(db))
}

fn message(
    account_id: &str,
    message_id: &str,
    segments: Vec<ContentSegment>,
) -> InboundMessageEnvelope {
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, account_id, message_id).unwrap(),
        ConversationRef::new(ConversationKind::Group, "group-1").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "sender-1").unwrap(),
        1_800_000_000,
        "@user 请确认",
        segments,
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn mysql_store_is_idempotent_and_resolves_reply_mentions() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let store = build_mysql_inbound_event_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let account_a = format!("account-a-{run_id}");
    let account_b = format!("account-b-{run_id}");

    let parent = message(&account_a, "message-1", Vec::new());
    let accepted_parent = store.insert_message_if_absent(&parent).await.unwrap();
    let parent_id = match accepted_parent {
        IngestMessageOutcome::Accepted {
            source_event_id, ..
        } => source_event_id,
        outcome => panic!("expected accepted parent, got {outcome:?}"),
    };

    let duplicate = store.insert_message_if_absent(&parent).await.unwrap();
    assert_eq!(duplicate.source_event_id(), &parent_id);
    assert!(matches!(duplicate, IngestMessageOutcome::Duplicate { .. }));

    let reply = message(
        &account_a,
        "message-2",
        vec![
            ContentSegment::Mention {
                actor_id: "member-2".into(),
            },
            ContentSegment::MentionAll,
            ContentSegment::Reply {
                platform_message_id: "message-1".into(),
            },
        ],
    );
    let accepted_reply = store.insert_message_if_absent(&reply).await.unwrap();
    let reply_id = match accepted_reply {
        IngestMessageOutcome::Accepted {
            source_event_id,
            reply_to_event_id,
        } => {
            assert_eq!(reply_to_event_id.as_ref(), Some(&parent_id));
            source_event_id
        }
        outcome => panic!("expected accepted reply, got {outcome:?}"),
    };

    let other_account = message(&account_b, "message-1", Vec::new());
    assert!(matches!(
        store
            .insert_message_if_absent(&other_account)
            .await
            .unwrap(),
        IngestMessageOutcome::Accepted { .. }
    ));

    let event_count = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT COUNT(*) AS count \
             FROM secretary_source_events event \
             INNER JOIN secretary_accounts account ON account.id = event.account_id \
             WHERE account.platform_account_id IN (?, ?)",
            [account_a.into(), account_b.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "count")
        .unwrap();
    assert_eq!(event_count, 3);

    let content = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT mentioned_actor_ids, mention_all FROM secretary_message_contents WHERE source_event_id = ?",
            [reply_id.as_str().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let mentioned = content
        .try_get::<serde_json::Value>("", "mentioned_actor_ids")
        .unwrap();
    let mention_all = content.try_get::<bool>("", "mention_all").unwrap();
    assert_eq!(mentioned, serde_json::json!(["member-2"]));
    assert!(mention_all);
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn mysql_store_never_persists_unknown_raw_payload() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let store = build_mysql_inbound_event_store(db.clone());
    let marker = "https://example.invalid/file?token=secret-token";
    let event = message(
        &format!("unknown-{}", Uuid::new_v4().simple()),
        "unknown-segment",
        vec![ContentSegment::Unknown {
            protocol_value: "unknown:future_card".into(),
        }],
    );
    let source_event_id = store
        .insert_message_if_absent(&event)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let segments = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(segments AS CHAR) AS value FROM secretary_message_contents WHERE source_event_id = ?",
            [source_event_id.as_str().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "value")
        .unwrap();

    assert!(segments.contains("unknown:future_card"));
    assert!(!segments.contains(marker));
    assert!(!segments.contains("secret-token"));
    assert!(!segments.contains("message text"));
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn mysql_store_tracks_connection_cursor_and_uncertain_gap() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let store = build_mysql_inbound_event_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("continuity-{run_id}");
    let account = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();

    let first_epoch = store.begin_connection(&account).await.unwrap();
    store.mark_connection_connected(&first_epoch).await.unwrap();
    let overflow_gap = store
        .mark_connection_uncertain(&first_epoch, IngestionGapReason::QueueOverflow)
        .await
        .unwrap();
    let repeated_overflow_gap = store
        .mark_connection_uncertain(&first_epoch, IngestionGapReason::QueueOverflow)
        .await
        .unwrap();
    assert_eq!(overflow_gap, repeated_overflow_gap);

    let observed = message(&account_id, "message-1", Vec::new()).observed_in(first_epoch.clone());
    let accepted = store.insert_message_if_absent(&observed).await.unwrap();
    let source_event_id = accepted.source_event_id().as_str().to_owned();

    let cursor_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value \
         FROM secretary_ingestion_cursors c \
         INNER JOIN secretary_accounts account ON account.id = c.account_id \
         WHERE account.platform_account_id = ?",
        [&account_id],
    )
    .await;
    assert_eq!(
        cursor_count, 2,
        "account and conversation cursors must exist"
    );

    let linked_event_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_event_ingestion \
         WHERE source_event_id = ? AND connection_epoch_id = ?",
        [source_event_id.as_str(), first_epoch.as_str()],
    )
    .await;
    assert_eq!(linked_event_count, 1);

    let first_gap = store
        .finish_connection(&first_epoch, ConnectionEndReason::TransportError)
        .await
        .unwrap()
        .expect("a connected epoch must create an uncertain gap");
    assert_eq!(first_gap, overflow_gap);
    let repeated_finish = store
        .finish_connection(&first_epoch, ConnectionEndReason::TransportError)
        .await
        .unwrap()
        .expect("finishing an epoch twice must return its existing gap");
    assert_eq!(first_gap, repeated_finish);

    let open_gap_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_ingestion_gaps \
         WHERE gap_id = ? AND status = 'uncertain' AND reason = 'queue_overflow' \
         AND gap_ended_at IS NULL",
        [first_gap.as_str()],
    )
    .await;
    assert_eq!(open_gap_count, 1);

    let second_epoch = store.begin_connection(&account).await.unwrap();
    store
        .mark_connection_connected(&second_epoch)
        .await
        .unwrap();
    let closed_but_unverified_gap_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_ingestion_gaps \
         WHERE gap_id = ? AND status = 'uncertain' AND gap_ended_at IS NOT NULL",
        [first_gap.as_str()],
    )
    .await;
    assert_eq!(closed_but_unverified_gap_count, 1);

    assert!(
        store
            .finish_connection(&second_epoch, ConnectionEndReason::ProcessShutdown)
            .await
            .unwrap()
            .is_some(),
        "shutdown also creates an uncertain window until the next verified backfill"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn gap_freezes_empty_evidence_on_first_write() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let directory = build_mysql_directory_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("empty-freeze-{run_id}");
    let account = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();

    let epoch = inbound.begin_connection(&account).await.unwrap();
    inbound.mark_connection_connected(&epoch).await.unwrap();
    let gap = inbound
        .mark_connection_uncertain(&epoch, IngestionGapReason::QueueOverflow)
        .await
        .unwrap();

    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_gap_boundaries WHERE gap_id = ?",
            [gap.as_str()],
        )
        .await,
        0
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_directory_gap_freeze \
             WHERE gap_id = ? AND snapshot_id IS NULL",
            [gap.as_str()],
        )
        .await,
        1,
        "an empty directory snapshot must still be frozen explicitly"
    );

    inbound
        .insert_message_if_absent(
            &message(&account_id, "late-message", Vec::new()).observed_in(epoch.clone()),
        )
        .await
        .unwrap();
    directory
        .snapshot_directory(&DirectorySnapshot {
            snapshot_id: DirectorySnapshotId::new(Uuid::new_v4().to_string()).unwrap(),
            account: account.clone(),
            source_api: DirectorySourceApi::FriendGroupRecent,
            status: DirectoryStatus::KnownScopesComplete,
            evidence: DirectoryEvidence::default(),
            scopes: Vec::new(),
            created_at_unix_secs: 1_800_000_100,
        })
        .await
        .unwrap();

    let duplicate = inbound
        .mark_connection_uncertain(&epoch, IngestionGapReason::QueueOverflow)
        .await
        .unwrap();
    assert_eq!(duplicate, gap);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_gap_boundaries WHERE gap_id = ?",
            [gap.as_str()],
        )
        .await,
        0,
        "later cursors must not be added to an existing Gap"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_directory_gap_freeze \
             WHERE gap_id = ? AND snapshot_id IS NULL",
            [gap.as_str()],
        )
        .await,
        1,
        "later directory snapshots must not replace an empty first-write freeze"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn owner_approved_thread_mutations_are_logical_idempotent_and_account_scoped() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let managed_account = format!("mutation-managed-{run_id}");

    let fixtures = [
        (
            "merge-a",
            ConversationKind::Group,
            "mutation-group-a",
            30_000,
        ),
        (
            "merge-b",
            ConversationKind::Private,
            "mutation-private-b",
            31_000,
        ),
        (
            "split-a",
            ConversationKind::Group,
            "mutation-split-group",
            40_000,
        ),
        (
            "split-b",
            ConversationKind::Group,
            "mutation-split-group",
            40_100,
        ),
    ];
    let mut event_ids = Vec::new();
    for (message_id, kind, conversation_id, occurred_at) in fixtures {
        let envelope = InboundMessageEnvelope::new(
            SourceMessageRef::new(MessageSource::NapCat, &managed_account, message_id).unwrap(),
            ConversationRef::new(kind, conversation_id).unwrap(),
            VerifiedActor::new(VerifiedActorKind::External, "member").unwrap(),
            occurred_at,
            message_id,
            Vec::new(),
        )
        .unwrap();
        event_ids.push(
            inbound
                .insert_message_if_absent(&envelope)
                .await
                .unwrap()
                .source_event_id()
                .clone(),
        );
    }

    let projection = ThreadProjectionUseCase::new(
        build_mysql_thread_projection_store(db.clone()),
        DeterministicThreadPlanner::new(DeterministicThreadPolicy::new(300).unwrap()),
        100,
        60,
        300,
    )
    .unwrap();
    while projection.run_once().await.unwrap().is_some() {}

    let merge_thread_a = scalar_string(
        &db,
        "SELECT thread_id AS value FROM secretary_thread_events WHERE source_event_id = ?",
        [event_ids[0].as_str()],
    )
    .await
    .unwrap();
    let merge_thread_b = scalar_string(
        &db,
        "SELECT thread_id AS value FROM secretary_thread_events WHERE source_event_id = ?",
        [event_ids[1].as_str()],
    )
    .await
    .unwrap();
    let split_thread = scalar_string(
        &db,
        "SELECT thread_id AS value FROM secretary_thread_events WHERE source_event_id = ?",
        [event_ids[2].as_str()],
    )
    .await
    .unwrap();
    assert_ne!(merge_thread_a, merge_thread_b);
    assert_eq!(
        split_thread,
        scalar_string(
            &db,
            "SELECT thread_id AS value FROM secretary_thread_events WHERE source_event_id = ?",
            [event_ids[3].as_str()],
        )
        .await
        .unwrap()
    );

    let command_account = format!("mutation-control-{run_id}");
    let owner_command = InboundMessageEnvelope::new(
        SourceMessageRef::new(
            MessageSource::QqOpenPlatform,
            &command_account,
            "mutation-owner-command",
        )
        .unwrap(),
        ConversationRef::new(ConversationKind::OwnerControl, "owner-control").unwrap(),
        VerifiedActor::new(VerifiedActorKind::Owner, "owner").unwrap(),
        50_000,
        "批准线程调整",
        Vec::new(),
    )
    .unwrap();
    let command_id = inbound
        .insert_message_if_absent(&owner_command)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_owner_bindings \
         (binding_id, managed_account_id, command_account_id, owner_actor_id, status) \
         SELECT ?, managed.id, command.id, 'owner', 'active' \
         FROM secretary_accounts managed JOIN secretary_accounts command \
         WHERE managed.source_channel = 'napcat' AND managed.platform_account_id = ? \
         AND command.source_channel = 'qq_open_platform' AND command.platform_account_id = ?",
        [
            Uuid::new_v4().to_string().into(),
            managed_account.clone().into(),
            command_account.into(),
        ],
    ))
    .await
    .unwrap();

    let store = build_mysql_thread_mutation_store(db.clone());
    let merge_impact = ThreadMutationImpact {
        proposal_id: ThreadMutationProposalId::generate(),
        kind: ThreadMutationKind::Merge,
        account: SourceAccountRef::new(MessageSource::NapCat, &managed_account).unwrap(),
        thread_ids: vec![
            EventThreadId::new(&merge_thread_a).unwrap(),
            EventThreadId::new(&merge_thread_b).unwrap(),
        ],
        affected_event_count: 2,
        affected_conversation_count: 2,
        affected_source_event_ids: event_ids[..2].to_vec(),
        reason: "Owner 确认两个跨会话线程属于同一事项".into(),
    };
    let mutation_state = ThreadMutationUseCase::new(store.clone())
        .prepare(merge_impact.clone())
        .await
        .unwrap();
    assert!(
        store
            .authorize_resume(&ThreadMutationResumeInput {
                proposal_id: merge_impact.proposal_id.clone(),
                decision: ThreadMutationDecision::Approve,
                command_source_event_id: event_ids[0].clone(),
            })
            .await
            .is_err(),
        "a NapCat observation must not authorize a mutation"
    );
    let first_runtime = thread_mutation_runtime(db.clone(), store.clone());
    let suspended = match first_runtime
        .run_checkpointed(
            AgentState::new(mutation_state),
            RunBudget::new(NonZeroU32::new(4).unwrap(), Duration::from_secs(10)),
        )
        .await
        .unwrap()
    {
        GraphExecutionResult::Suspended(value) => value,
        GraphExecutionResult::Completed(_) => panic!("thread mutation must suspend for approval"),
    };
    let checkpoint_id = suspended.checkpoint().id();
    drop(first_runtime);

    let resumed_store = build_mysql_thread_mutation_store(db.clone());
    let resumed_runtime = thread_mutation_runtime(db.clone(), resumed_store);
    let completed = match resumed_runtime
        .resume(
            checkpoint_id,
            ThreadMutationResumeInput {
                proposal_id: merge_impact.proposal_id.clone(),
                decision: ThreadMutationDecision::Approve,
                command_source_event_id: command_id.clone(),
            },
        )
        .await
        .unwrap()
    {
        GraphExecutionResult::Completed(value) => value,
        GraphExecutionResult::Suspended(_) => panic!("approved mutation must complete"),
    };
    assert_eq!(completed.effect_receipts.len(), 1);
    let merge_effect_id = completed.effect_receipts[0].effect_id.to_string();
    let merge_effect = ThreadMutationEffect {
        proposal_id: merge_impact.proposal_id.clone(),
        kind: ThreadMutationKind::Merge,
    };
    assert!(
        !store
            .apply_effect(&merge_effect, &merge_effect_id)
            .await
            .unwrap()
            .changed
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_thread_mutation_checkpoints \
             WHERE checkpoint_id = ? AND checkpoint_status = 'consumed'",
            [&checkpoint_id.to_string()],
        )
        .await,
        1,
        "resume must consume the durable checkpoint exactly once"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(DISTINCT thread_id) AS value FROM secretary_effective_thread_events \
             WHERE source_event_id IN (?, ?)",
            [event_ids[0].as_str(), event_ids[1].as_str()],
        )
        .await,
        1
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(DISTINCT thread_id) AS value FROM secretary_thread_events \
             WHERE source_event_id IN (?, ?)",
            [event_ids[0].as_str(), event_ids[1].as_str()],
        )
        .await,
        2,
        "logical merge must not rewrite original membership"
    );

    let reply_after_merge = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &managed_account, "reply-after-merge")
            .unwrap(),
        ConversationRef::new(ConversationKind::Private, "mutation-private-b").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "member").unwrap(),
        32_000,
        "继续处理",
        vec![ContentSegment::Reply {
            platform_message_id: "merge-b".into(),
        }],
    )
    .unwrap();
    let reply_after_merge_id = inbound
        .insert_message_if_absent(&reply_after_merge)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    while projection.run_once().await.unwrap().is_some() {}
    assert_eq!(
        scalar_string(
            &db,
            "SELECT thread_id AS value FROM secretary_thread_events WHERE source_event_id = ?",
            [reply_after_merge_id.as_str()],
        )
        .await
        .unwrap(),
        merge_thread_a,
        "new replies to a merged alias must project directly into the canonical thread"
    );

    let merge_revert_command = InboundMessageEnvelope::new(
        SourceMessageRef::new(
            MessageSource::QqOpenPlatform,
            format!("mutation-control-{run_id}"),
            "mutation-revert-merge",
        )
        .unwrap(),
        ConversationRef::new(ConversationKind::OwnerControl, "owner-control").unwrap(),
        VerifiedActor::new(VerifiedActorKind::Owner, "owner").unwrap(),
        50_100,
        "撤销线程合并",
        Vec::new(),
    )
    .unwrap();
    let merge_revert_command_id = inbound
        .insert_message_if_absent(&merge_revert_command)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let merge_revert = ThreadMutationRevertInput {
        proposal_id: merge_impact.proposal_id.clone(),
        command_source_event_id: merge_revert_command_id,
        reason: "Owner 发现两个会话并非同一事项".into(),
    };
    let revert_use_case = ThreadMutationRevertUseCase::new(store.clone());
    assert!(revert_use_case.revert(&merge_revert).await.unwrap().changed);
    assert!(!revert_use_case.revert(&merge_revert).await.unwrap().changed);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(DISTINCT thread_id) AS value FROM secretary_effective_thread_events \
             WHERE source_event_id IN (?, ?)",
            [event_ids[0].as_str(), event_ids[1].as_str()],
        )
        .await,
        2,
        "reverting a merge must restore the original affected event threads"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_thread_semantic_invalidations \
             WHERE proposal_id = ?",
            [merge_impact.proposal_id.as_str()],
        )
        .await,
        4,
        "apply and revert must invalidate both merge threads"
    );

    let split_impact = ThreadMutationImpact {
        proposal_id: ThreadMutationProposalId::generate(),
        kind: ThreadMutationKind::Split,
        account: SourceAccountRef::new(MessageSource::NapCat, &managed_account).unwrap(),
        thread_ids: vec![EventThreadId::new(&split_thread).unwrap()],
        affected_event_count: 1,
        affected_conversation_count: 1,
        affected_source_event_ids: vec![event_ids[2].clone()],
        reason: "Owner 确认其中一条消息属于另一个事项".into(),
    };
    ThreadMutationUseCase::new(store.clone())
        .prepare(split_impact.clone())
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_semantic_state (thread_id, attempts) VALUES (?, 1)",
        [split_thread.clone().into()],
    ))
    .await
    .unwrap();
    store
        .authorize_resume(&ThreadMutationResumeInput {
            proposal_id: split_impact.proposal_id.clone(),
            decision: ThreadMutationDecision::Approve,
            command_source_event_id: command_id,
        })
        .await
        .unwrap();
    store
        .apply_effect(
            &ThreadMutationEffect {
                proposal_id: split_impact.proposal_id.clone(),
                kind: ThreadMutationKind::Split,
            },
            &format!("split-effect-{run_id}"),
        )
        .await
        .unwrap();
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_thread_semantic_state WHERE thread_id = ?",
            [&split_thread],
        )
        .await,
        0,
        "applying a split must reset stale semantic cursors"
    );
    let selected_effective = scalar_string(
        &db,
        "SELECT thread_id AS value FROM secretary_effective_thread_events WHERE source_event_id = ?",
        [event_ids[2].as_str()],
    )
    .await
    .unwrap();
    let untouched_effective = scalar_string(
        &db,
        "SELECT thread_id AS value FROM secretary_effective_thread_events WHERE source_event_id = ?",
        [event_ids[3].as_str()],
    )
    .await
    .unwrap();
    assert_eq!(selected_effective, split_impact.proposal_id.as_str());
    assert_eq!(untouched_effective, split_thread);

    let split_revert_command = InboundMessageEnvelope::new(
        SourceMessageRef::new(
            MessageSource::QqOpenPlatform,
            format!("mutation-control-{run_id}"),
            "mutation-revert-split",
        )
        .unwrap(),
        ConversationRef::new(ConversationKind::OwnerControl, "owner-control").unwrap(),
        VerifiedActor::new(VerifiedActorKind::Owner, "owner").unwrap(),
        50_200,
        "撤销线程拆分",
        Vec::new(),
    )
    .unwrap();
    let split_revert_command_id = inbound
        .insert_message_if_absent(&split_revert_command)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    assert!(
        revert_use_case
            .revert(&ThreadMutationRevertInput {
                proposal_id: split_impact.proposal_id.clone(),
                command_source_event_id: split_revert_command_id,
                reason: "Owner 确认原拆分判断错误".into(),
            })
            .await
            .unwrap()
            .changed
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT thread_id AS value FROM secretary_effective_thread_events WHERE source_event_id = ?",
            [event_ids[2].as_str()],
        )
        .await
        .unwrap(),
        split_thread,
        "reverting a split must restore the original effective thread"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_thread_events te \
             JOIN secretary_source_events event ON event.source_event_id = te.source_event_id \
             JOIN secretary_accounts account ON account.id = event.account_id \
             WHERE account.platform_account_id = ?",
            [&managed_account],
        )
        .await,
        5,
        "split must preserve every original membership row"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn structured_memory_is_source_backed_versioned_private_and_expirable() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("memory-account-{run_id}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();

    let normal = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &account_id, "memory-normal").unwrap(),
        ConversationRef::new(ConversationKind::Group, "memory-group").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "alice").unwrap(),
        60_000,
        "我负责报价，明天下午发送报价单",
        Vec::new(),
    )
    .unwrap();
    let normal_id = inbound
        .insert_message_if_absent(&normal)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let excluded = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &account_id, "memory-excluded").unwrap(),
        ConversationRef::new(ConversationKind::Private, "memory-private").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "private-person").unwrap(),
        60_100,
        "这条消息永不进入长期记忆",
        Vec::new(),
    )
    .unwrap();
    let excluded_id = inbound
        .insert_message_if_absent(&excluded)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_conversations conversation \
         JOIN secretary_accounts account ON account.id = conversation.account_id \
         SET conversation.memory_mode = 'never_long_term' \
         WHERE account.platform_account_id = ? \
         AND conversation.platform_conversation_id = 'memory-private'",
        [account_id.clone().into()],
    ))
    .await
    .unwrap();

    let store = build_mysql_memory_store(db.clone());
    let use_case = MemoryUseCase::new(store.clone());
    let person = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: managed.clone(),
        subject_key: "person:alice".into(),
        payload: MemoryPayload::Person(PersonMemory {
            person: ThreadActorRef {
                account: managed.clone(),
                actor_id: "alice".into(),
            },
            relationship: Some("项目协作者".into()),
            responsibilities: vec!["负责报价".into()],
            communication_preferences: vec!["使用简短确认".into()],
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_000,
        source_event_ids: vec![normal_id.clone()],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    assert!(use_case.remember(&person).await.unwrap().changed);
    assert!(!use_case.remember(&person).await.unwrap().changed);
    let conflicting_person = MemoryFact {
        fact_id: MemoryFactId::generate(),
        payload: MemoryPayload::Person(PersonMemory {
            relationship: Some("客户联系人".into()),
            ..match &person.payload {
                MemoryPayload::Person(value) => value.clone(),
                _ => unreachable!(),
            }
        }),
        ..person.clone()
    };
    assert!(
        use_case.remember(&conflicting_person).await.is_err(),
        "a conflicting active fact must reread sources and explicitly supersede"
    );

    let project = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: managed.clone(),
        subject_key: "project:quote".into(),
        payload: MemoryPayload::Project(ProjectMemory {
            project_key: "quote".into(),
            goal: "完成报价单并发送".into(),
            member_actor_ids: vec!["alice".into()],
            progress: Some("准备中".into()),
            decision_ids: Vec::new(),
            risks: Vec::new(),
            blockers: Vec::new(),
            artifact_refs: vec!["artifact:quote-draft".into()],
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 8_500,
        source_event_ids: vec![normal_id.clone()],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    use_case.remember(&project).await.unwrap();
    let revised_project = MemoryFact {
        fact_id: MemoryFactId::generate(),
        payload: MemoryPayload::Project(ProjectMemory {
            project_key: "quote".into(),
            goal: "完成报价单并发送".into(),
            member_actor_ids: vec!["alice".into()],
            progress: Some("等待 Owner 确认".into()),
            decision_ids: Vec::new(),
            risks: Vec::new(),
            blockers: vec!["等待价格确认".into()],
            artifact_refs: vec!["artifact:quote-draft".into()],
        }),
        supersedes_fact_id: Some(project.fact_id.clone()),
        ..project.clone()
    };
    use_case.remember(&revised_project).await.unwrap();

    let commitment = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: managed.clone(),
        subject_key: "commitment:send-quote".into(),
        payload: MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: managed.clone(),
                actor_id: "alice".into(),
            },
            beneficiary: ThreadActorRef {
                account: managed.clone(),
                actor_id: "owner".into(),
            },
            action: "发送报价单".into(),
            due_at_unix_secs: Some(70_000),
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_500,
        source_event_ids: vec![normal_id.clone()],
        valid_until_unix_secs: Some(80_000),
        supersedes_fact_id: None,
    };
    use_case.remember(&commitment).await.unwrap();
    assert_eq!(store.expire_due(80_000, 100).await.unwrap(), 1);

    let active = use_case.active(&managed, 20).await.unwrap();
    assert_eq!(active.len(), 2);
    assert!(active.iter().any(|fact| fact.fact_id == person.fact_id));
    assert!(
        active
            .iter()
            .any(|fact| fact.fact_id == revised_project.fact_id)
    );
    assert!(!active.iter().any(|fact| fact.fact_id == project.fact_id));

    let blocker_follow_up = personal_secretary::FollowUpUseCase::new(
        build_mysql_follow_up_store(db.clone()),
        store.clone(),
    );
    let blocker_report = blocker_follow_up
        .scan(2_000_000_000, 604_800, 14_400, 86_400, 100)
        .await
        .unwrap();
    assert_eq!(blocker_report.project_blockers_materialized, 1);
    assert_eq!(blocker_report.notification_candidates_created, 1);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value
             FROM secretary_notification_candidates candidate
             JOIN secretary_follow_up_items item ON item.follow_up_id = candidate.source_id
             WHERE candidate.source_kind = 'follow_up'
               AND item.reason_code = 'project_blocked'
               AND JSON_UNQUOTE(JSON_EXTRACT(candidate.match_key_json, '$.event_kind.value')) = 'project_blocked'",
            [],
        )
        .await,
        1,
        "an unresolved project blocker must enter the unified policy queue"
    );

    let private_fact = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: managed.clone(),
        subject_key: "person:private".into(),
        payload: MemoryPayload::Person(PersonMemory {
            person: ThreadActorRef {
                account: managed,
                actor_id: "private-person".into(),
            },
            relationship: None,
            responsibilities: Vec::new(),
            communication_preferences: Vec::new(),
        }),
        status: MemoryFactStatus::Proposed,
        confidence_bps: 5_000,
        source_event_ids: vec![excluded_id],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    assert!(use_case.remember(&private_fact).await.is_err());
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_memory_fact_sources source \
             JOIN secretary_memory_facts fact ON fact.fact_id = source.fact_id \
             JOIN secretary_accounts account ON account.id = fact.account_id \
             WHERE account.platform_account_id = ?",
            [&account_id],
        )
        .await,
        4,
        "every persisted memory version must retain its source event"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn memory_evidence_owner_delete_and_follow_up_outbox_form_a_closed_loop() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("follow-up-account-{run_id}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();
    let source = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &account_id, "commitment-source").unwrap(),
        ConversationRef::new(ConversationKind::Group, "delivery-group").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "alice").unwrap(),
        69_000,
        "我会在今天发送报价单",
        Vec::new(),
    )
    .unwrap();
    let source_id = inbound
        .insert_message_if_absent(&source)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let memory_store = build_mysql_memory_store(db.clone());
    let memory = MemoryUseCase::new(memory_store.clone());
    let fact = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: managed.clone(),
        subject_key: "commitment:quote-delivery".into(),
        payload: MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: managed.clone(),
                actor_id: "alice".into(),
            },
            beneficiary: ThreadActorRef {
                account: managed,
                actor_id: "owner".into(),
            },
            action: "发送报价单".into(),
            due_at_unix_secs: Some(70_000),
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_500,
        source_event_ids: vec![source_id],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory.remember(&fact).await.unwrap();
    let evidence = memory.evidence(&fact.fact_id, 12).await.unwrap().unwrap();
    assert_eq!(evidence.sources.len(), 1);
    assert_eq!(
        evidence.sources[0].excerpt,
        "我会在今天发送报价单".chars().take(12).collect::<String>()
    );

    let follow_up = personal_secretary::FollowUpUseCase::new(
        build_mysql_follow_up_store(db.clone()),
        memory_store,
    );
    let report = follow_up
        .scan(70_000, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    assert_eq!(report.commitments_materialized, 1);
    assert_eq!(report.notification_candidates_created, 1);
    assert_eq!(report.notification_evaluation_requests_created, 1);
    let replay = follow_up
        .scan(70_000, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    assert_eq!(replay.commitments_materialized, 0);
    assert_eq!(replay.notification_candidates_created, 0);
    assert_eq!(replay.notification_evaluation_requests_created, 0);

    let command_account = format!("follow-up-control-{run_id}");
    let owner_command = InboundMessageEnvelope::new(
        SourceMessageRef::new(
            MessageSource::QqOpenPlatform,
            &command_account,
            "delete-memory-command",
        )
        .unwrap(),
        ConversationRef::new(ConversationKind::OwnerControl, "owner-control").unwrap(),
        VerifiedActor::new(VerifiedActorKind::Owner, "owner").unwrap(),
        70_100,
        "删除这条承诺记忆",
        Vec::new(),
    )
    .unwrap();
    let command_id = inbound
        .insert_message_if_absent(&owner_command)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_owner_bindings \
         (binding_id, managed_account_id, command_account_id, owner_actor_id, status) \
         SELECT ?, managed.id, command.id, 'owner', 'active' \
         FROM secretary_accounts managed JOIN secretary_accounts command \
         WHERE managed.source_channel = 'napcat' AND managed.platform_account_id = ? \
         AND command.source_channel = 'qq_open_platform' AND command.platform_account_id = ?",
        [
            Uuid::new_v4().to_string().into(),
            account_id.clone().into(),
            command_account.into(),
        ],
    ))
    .await
    .unwrap();
    let mode_receipt = memory
        .set_conversation_mode(&ConversationMemoryModeInput {
            account: fact.account.clone(),
            conversation: ConversationRef::new(ConversationKind::Group, "delivery-group").unwrap(),
            command_source_event_id: command_id.clone(),
            mode: ContentTrustLevel::NeverLongTerm,
        })
        .await
        .unwrap();
    assert!(mode_receipt.changed);
    assert_eq!(mode_receipt.previous_mode, ContentTrustLevel::Normal);
    assert_eq!(mode_receipt.current_mode, ContentTrustLevel::NeverLongTerm);
    assert!(
        !memory
            .set_conversation_mode(&ConversationMemoryModeInput {
                account: fact.account.clone(),
                conversation: ConversationRef::new(ConversationKind::Group, "delivery-group")
                    .unwrap(),
                command_source_event_id: command_id.clone(),
                mode: ContentTrustLevel::NeverLongTerm,
            })
            .await
            .unwrap()
            .changed
    );
    let deletion = MemoryDeleteInput {
        fact_id: fact.fact_id.clone(),
        command_source_event_id: command_id,
        reason: "Owner 确认该承诺无效".into(),
    };
    assert!(memory.delete_derived(&deletion).await.unwrap().changed);
    assert!(!memory.delete_derived(&deletion).await.unwrap().changed);
    let report = follow_up
        .scan(70_101, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    assert_eq!(report.items_reconciled, 1);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_outbox outbox \
             JOIN secretary_follow_up_items item ON item.follow_up_id = outbox.follow_up_id \
             WHERE item.source_memory_fact_id = ?",
            [fact.fact_id.as_str()],
        )
        .await,
        0,
        "follow-up scans must not create legacy Outbox rows before policy evaluation"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn unanswered_external_question_becomes_policy_candidate_then_resolves_on_own_reply() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let account_id = format!("response-account-{suffix}");
    let question = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &account_id, "question-1").unwrap(),
        ConversationRef::new(ConversationKind::Group, "response-group").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "customer").unwrap(),
        1_000,
        "报价单今天能发给我吗？",
        Vec::new(),
    )
    .unwrap();
    let question_event_id = inbound
        .insert_message_if_absent(&question)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let thread_id = Uuid::new_v4().to_string();
    let question_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_event_threads \
         (thread_id, account_id, status, root_event_id, latest_event_id, \
          opened_at_unix_secs, latest_occurred_at_unix_secs) \
         SELECT ?, account_id, 'open', source_event_id, source_event_id, \
                occurred_at_unix_secs, occurred_at_unix_secs \
         FROM secretary_source_events WHERE source_event_id = ?",
        [thread_id.clone().into(), question_event_id.as_str().into()],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_events (source_event_id, thread_id) VALUES (?, ?)",
        [question_event_id.as_str().into(), thread_id.clone().into()],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_open_questions \
         (question_id, thread_id, raised_by_channel, raised_by_account, \
          raised_by_actor_id, question, status, confidence_bps) \
         VALUES (?, ?, 'napcat', ?, 'customer', '报价单今天能发给我吗？', 'open', 9500)",
        [
            question_id.clone().into(),
            thread_id.clone().into(),
            account_id.clone().into(),
        ],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_question_sources (question_id, source_event_id) VALUES (?, ?)",
        [question_id.clone().into(), question_event_id.as_str().into()],
    ))
    .await
    .unwrap();

    let follow_up = personal_secretary::FollowUpUseCase::new(
        build_mysql_follow_up_store(db.clone()),
        build_mysql_memory_store(db.clone()),
    );
    let report = follow_up
        .scan(15_401, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    assert_eq!(report.response_expectations_materialized, 1);
    assert_eq!(report.notification_candidates_created, 1);
    assert_eq!(report.notification_evaluation_requests_created, 1);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_candidates \
             WHERE source_kind = 'response_expectation'",
            [],
        )
        .await,
        1,
        "one overdue response expectation must create one policy candidate"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_outbox",
            [],
        )
        .await,
        0,
        "response expectation scan must not bypass policy evaluation"
    );

    let command_account_id = format!("response-control-{suffix}");
    let owner_command = InboundMessageEnvelope::new(
        SourceMessageRef::new(
            MessageSource::QqOpenPlatform,
            &command_account_id,
            "response-owner-command",
        )
        .unwrap(),
        ConversationRef::new(ConversationKind::OwnerControl, "response-owner-control").unwrap(),
        VerifiedActor::new(VerifiedActorKind::Owner, "owner").unwrap(),
        15_500,
        "检查需要我回复的消息",
        Vec::new(),
    )
    .unwrap();
    inbound
        .insert_message_if_absent(&owner_command)
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_owner_bindings \
         (binding_id, managed_account_id, command_account_id, owner_actor_id, status) \
         SELECT ?, managed.id, command.id, 'owner', 'active' \
         FROM secretary_accounts managed JOIN secretary_accounts command \
         WHERE managed.source_channel = 'napcat' AND managed.platform_account_id = ? \
           AND command.source_channel = 'qq_open_platform' \
           AND command.platform_account_id = ?",
        [
            Uuid::new_v4().to_string().into(),
            account_id.clone().into(),
            command_account_id.into(),
        ],
    ))
    .await
    .unwrap();
    let policy = NotificationPolicyUseCase::new(
        build_mysql_notification_policy_store(db.clone()),
        Arc::new(SystemClock),
    );
    let evaluation = policy
        .evaluate_next("response-expectation-test", 60, |snapshot| {
            NotificationPolicyEvaluator.evaluate(&snapshot.evaluation_input(15_501).unwrap())
        })
        .await
        .unwrap();
    assert_eq!(evaluation, Some(EvaluationCommitResult::Applied));
    let managed = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();
    let claimed = follow_up
        .claim_due_notification(&managed, i64::MAX, 60)
        .await
        .unwrap()
        .expect("remind decision must materialize an Owner notification");
    match &claimed.content {
        OwnerNotificationContent::ResponseExpectation {
            question_id: delivered_question_id,
            thread_id: delivered_thread_id,
            question_excerpt,
            ..
        } => {
            assert_eq!(delivered_question_id, &question_id);
            assert_eq!(delivered_thread_id, &thread_id);
            assert!(question_excerpt.contains("报价单"));
        }
        other => panic!("expected response expectation notification, got {other:?}"),
    }
    follow_up
        .mark_notification_failed(
            &claimed.notification_id,
            &claimed.lease_token,
            "test_not_sent",
            NotificationFailureKind::Permanent,
        )
        .await
        .unwrap();

    let reply = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &account_id, "owner-reply-1").unwrap(),
        ConversationRef::new(ConversationKind::Group, "response-group").unwrap(),
        VerifiedActor::new(VerifiedActorKind::OfficialBot, "managed-account").unwrap(),
        16_000,
        "可以，今天下班前发送。",
        Vec::new(),
    )
    .unwrap();
    let reply_event_id = inbound
        .insert_message_if_absent(&reply)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_events (source_event_id, thread_id) VALUES (?, ?)",
        [reply_event_id.as_str().into(), thread_id.into()],
    ))
    .await
    .unwrap();
    let resolved = follow_up
        .scan(16_001, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    assert_eq!(resolved.response_expectations_resolved, 1);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_response_expectations \
             WHERE expectation_status = 'resolved' AND source_question_id = ?",
            [question_id.as_str()],
        )
        .await,
        1,
        "an own reply in the same thread must resolve the expectation"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_outbox \
             WHERE delivery_status = 'suppressed' \
               AND last_error_code = 'response_already_resolved'",
            [],
        )
        .await,
        1,
        "resolved response expectations must suppress unsent notifications"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn notification_outbox_fences_leases_and_stops_on_unknown_commit() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("outbox-account-{run_id}");
    let account = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();
    let source = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &account_id, "outbox-source").unwrap(),
        ConversationRef::new(ConversationKind::Private, "owner-private").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "alice").unwrap(),
        80_000,
        "明天提交材料",
        Vec::new(),
    )
    .unwrap();
    let source_id = inbound
        .insert_message_if_absent(&source)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let memory_store = build_mysql_memory_store(db.clone());
    let memory = MemoryUseCase::new(memory_store.clone());
    let foreign_account_id = format!("outbox-foreign-{run_id}");
    let foreign_account =
        SourceAccountRef::new(MessageSource::NapCat, &foreign_account_id).unwrap();
    let foreign_source = InboundMessageEnvelope::new(
        SourceMessageRef::new(
            MessageSource::NapCat,
            &foreign_account_id,
            "foreign-outbox-source",
        )
        .unwrap(),
        ConversationRef::new(ConversationKind::Private, "foreign-private").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "bob").unwrap(),
        80_000,
        "明天提交另一份材料",
        Vec::new(),
    )
    .unwrap();
    let foreign_source_id = inbound
        .insert_message_if_absent(&foreign_source)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let foreign_fact = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: foreign_account.clone(),
        subject_key: "commitment:foreign-outbox".into(),
        payload: MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: foreign_account.clone(),
                actor_id: "bob".into(),
            },
            beneficiary: ThreadActorRef {
                account: foreign_account.clone(),
                actor_id: "foreign-owner".into(),
            },
            action: "提交其他账号材料".into(),
            due_at_unix_secs: Some(80_100),
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_500,
        source_event_ids: vec![foreign_source_id],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory.remember(&foreign_fact).await.unwrap();
    let first_fact = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: account.clone(),
        subject_key: "commitment:outbox-unknown".into(),
        payload: MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: account.clone(),
                actor_id: "alice".into(),
            },
            beneficiary: ThreadActorRef {
                account: account.clone(),
                actor_id: "owner".into(),
            },
            action: "提交第一份材料".into(),
            due_at_unix_secs: Some(80_100),
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_500,
        source_event_ids: vec![source_id.clone()],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory.remember(&first_fact).await.unwrap();
    let follow_up = personal_secretary::FollowUpUseCase::new(
        build_mysql_follow_up_store(db.clone()),
        memory_store.clone(),
    );
    follow_up
        .scan(80_100, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    // Task 7 后扫描只创建 Candidate/Request；此处显式构造两条历史 Outbox，
    // 保留该测试对旧投递状态机（含跨账号领取）的覆盖。
    let first_follow_up_id = scalar_string(
        &db,
        "SELECT item.follow_up_id AS value FROM secretary_follow_up_items item \
         JOIN secretary_accounts account ON account.id = item.account_id \
         WHERE account.source_channel = 'napcat' AND account.platform_account_id = ? \
         AND item.source_memory_fact_id = ?",
        [&account_id, first_fact.fact_id.as_str()],
    )
    .await
    .expect("first follow-up must be materialized");
    let foreign_follow_up_id = scalar_string(
        &db,
        "SELECT item.follow_up_id AS value FROM secretary_follow_up_items item \
         JOIN secretary_accounts account ON account.id = item.account_id \
         WHERE account.source_channel = 'napcat' AND account.platform_account_id = ? \
         AND item.source_memory_fact_id = ?",
        [&foreign_account_id, foreign_fact.fact_id.as_str()],
    )
    .await
    .expect("foreign follow-up must be materialized");
    for (notification_id, platform_account_id, follow_up_id) in [
        (
            Uuid::new_v4().to_string(),
            account_id.clone(),
            first_follow_up_id,
        ),
        (
            Uuid::new_v4().to_string(),
            foreign_account_id.clone(),
            foreign_follow_up_id,
        ),
    ] {
        let inserted = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO secretary_notification_outbox \
                 (notification_id, account_id, follow_up_id, scheduled_at_unix_secs, notification_kind, payload_json, delivery_status) \
                 SELECT ?, id, ?, 80100, 'owner_reminder', JSON_OBJECT('legacy_fixture', true), 'pending' \
                 FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
                [
                    notification_id.into(),
                    follow_up_id.into(),
                    platform_account_id.into(),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(
            inserted.rows_affected(),
            1,
            "legacy Outbox fixture must insert once"
        );
    }

    let first = follow_up
        .claim_due_notification(&account, 80_100, 60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.attempt, 1);
    let wrong_lease = personal_secretary::NotificationLeaseToken::generate();
    assert!(matches!(
        follow_up
            .mark_notification_delivered(
                &first.notification_id,
                &wrong_lease,
                "must-not-be-recorded"
            )
            .await,
        Err(personal_secretary::InboundEventStoreError::LeaseLost)
    ));
    follow_up
        .mark_notification_failed(
            &first.notification_id,
            &first.lease_token,
            "rate_limited",
            personal_secretary::NotificationFailureKind::Retryable,
        )
        .await
        .unwrap();
    assert!(
        follow_up
            .claim_due_notification(&account, 80_100, 60)
            .await
            .unwrap()
            .is_none(),
        "retryable failure must respect its backoff"
    );
    let retried = follow_up
        .claim_due_notification(&account, 2_000_000_000, 60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retried.notification_id, first.notification_id);
    assert_eq!(retried.attempt, 2);
    follow_up
        .mark_notification_failed(
            &retried.notification_id,
            &retried.lease_token,
            "ambiguous_post",
            personal_secretary::NotificationFailureKind::UnknownCommit,
        )
        .await
        .unwrap();
    assert!(
        follow_up
            .claim_due_notification(&account, 2_000_000_000, 60)
            .await
            .unwrap()
            .is_none(),
        "unknown commit must never be retried blindly"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT delivery_status AS value FROM secretary_notification_outbox WHERE notification_id = ?",
            [first.notification_id.as_str()],
        )
        .await
        .as_deref(),
        Some("unknown_commit")
    );

    let delivered_fact = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: account.clone(),
        subject_key: "commitment:outbox-delivered".into(),
        payload: MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: account.clone(),
                actor_id: "alice".into(),
            },
            beneficiary: ThreadActorRef {
                account: account.clone(),
                actor_id: "owner".into(),
            },
            action: "提交第二份材料".into(),
            due_at_unix_secs: Some(100_001),
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_500,
        source_event_ids: vec![source_id],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory.remember(&delivered_fact).await.unwrap();
    let scan = follow_up
        .scan(100_001, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    assert_eq!(scan.commitments_materialized, 1);
    assert_eq!(scan.notification_candidates_created, 1);
    assert_eq!(scan.notification_evaluation_requests_created, 1);
    let delivered_follow_up_id = scalar_exactly_one_string(
        &db,
        "SELECT item.follow_up_id AS value FROM secretary_follow_up_items item \
         JOIN secretary_accounts account ON account.id = item.account_id \
         WHERE account.source_channel = 'napcat' AND account.platform_account_id = ? \
         AND item.source_memory_fact_id = ?",
        [&account_id, delivered_fact.fact_id.as_str()],
        "delivered fact must materialize exactly one follow-up item",
    )
    .await;
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_candidates candidate \
             WHERE candidate.source_kind = 'follow_up' AND candidate.source_id = ?",
            [&delivered_follow_up_id],
        )
        .await,
        1,
        "follow-up scan must produce one policy Candidate"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_evaluation_requests request \
             JOIN secretary_notification_candidates candidate \
               ON candidate.notification_candidate_id = request.notification_candidate_id \
             WHERE candidate.source_kind = 'follow_up' AND candidate.source_id = ?",
            [&delivered_follow_up_id],
        )
        .await,
        1,
        "follow-up scan must produce one policy evaluation Request"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_outbox outbox \
             JOIN secretary_follow_up_items item ON item.follow_up_id = outbox.follow_up_id \
             JOIN secretary_accounts account ON account.id = outbox.account_id \
             WHERE account.source_channel = 'napcat' AND account.platform_account_id = ? \
             AND item.source_memory_fact_id = ?",
            [&account_id, delivered_fact.fact_id.as_str()],
        )
        .await,
        0,
        "FollowUp 扫描不得直接创建 legacy Outbox"
    );
    // legacy Outbox 仅是本测试投递状态机的显式 fixture，不属于扫描生产行为。
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_notification_outbox \
             (notification_id, account_id, follow_up_id, scheduled_at_unix_secs, notification_kind, payload_json, delivery_status) \
             SELECT ?, id, ?, 100001, 'owner_reminder', JSON_OBJECT('legacy_fixture', true), 'pending' \
             FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
            [
                Uuid::new_v4().to_string().into(),
                delivered_follow_up_id.into(),
                account_id.clone().into(),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(
        inserted.rows_affected(),
        1,
        "delivered legacy Outbox fixture must insert exactly one row"
    );
    let delivered = follow_up
        .claim_due_notification(&account, 100_001, 60)
        .await
        .unwrap()
        .unwrap();
    follow_up
        .mark_notification_delivered(
            &delivered.notification_id,
            &delivered.lease_token,
            "platform-message-1",
        )
        .await
        .unwrap();
    assert_eq!(
        scalar_string(
            &db,
            "SELECT platform_message_id AS value FROM secretary_notification_outbox WHERE notification_id = ?",
            [delivered.notification_id.as_str()],
        )
        .await
        .as_deref(),
        Some("platform-message-1")
    );
    assert!(
        follow_up
            .claim_due_notification(&foreign_account, 2_000_000_000, 60)
            .await
            .unwrap()
            .is_some(),
        "claiming one account must not consume another account's notification"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn agenda_due_outbox_is_idempotent_version_fenced_and_lease_fenced() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;

    let inbound = build_mysql_inbound_event_store(db.clone());
    let account_id = format!("agenda-outbox-{}", Uuid::new_v4().simple());
    let account = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();
    let command = message(&account_id, "agenda-command", Vec::new());
    let command_source_event_id = inbound
        .insert_message_if_absent(&command)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let run_id = Uuid::new_v4().to_string();
    let lease_token = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"INSERT INTO secretary_action_runs
           (run_id, account_id, command_source_event_id, command_text, conversation_id,
            occurred_at_unix_secs, timezone_offset_secs, timezone_name, recent_events_json,
            status, lease_token)
           SELECT ?, id, ?, '创建提醒', 'owner-private', 100000, 0, 'UTC', JSON_ARRAY(),
                  'running', ?
           FROM secretary_accounts
           WHERE source_channel = ? AND platform_account_id = ?"#,
        [
            run_id.clone().into(),
            command_source_event_id.as_str().into(),
            lease_token.clone().into(),
            MessageSource::NapCat.as_str().into(),
            account_id.clone().into(),
        ],
    ))
    .await
    .unwrap();

    let create = AgendaUseCase::new(
        build_mysql_agenda_store(db.clone()),
        Arc::new(FixedClock { now: 100_000 }),
    );
    let created = create
        .apply(&AgendaApplyRequest {
            account: account.clone(),
            command_source_event_id: command_source_event_id.clone(),
            run_id: run_id.clone(),
            effect_id: format!("agenda-create-{}", Uuid::new_v4()),
            proposal_id: Uuid::new_v4().to_string(),
            proposal_json: r#"{"kind":"create_reminder"}"#.into(),
            lease_token: lease_token.clone(),
            idempotency_key: format!("agenda-create-{run_id}"),
            mutation: AgendaMutation::Create {
                kind: AgendaItemKind::Reminder,
                title: "续费提醒".into(),
                scheduled_at_unix_secs: Some(100_001),
                timezone: "Asia/Shanghai".into(),
            },
        })
        .await
        .unwrap();
    assert_eq!(created.item.version, 1);

    let due = AgendaUseCase::new(
        build_mysql_agenda_store(db.clone()),
        Arc::new(FixedClock { now: 100_001 }),
    );
    let first_report = due.produce_due_notification_candidates(100).await.unwrap();
    assert_eq!(first_report.candidates_created, 1);
    assert_eq!(first_report.requests_created, 1);
    let repeated_report = due.produce_due_notification_candidates(100).await.unwrap();
    assert_eq!(repeated_report.candidates_created, 0);
    assert_eq!(repeated_report.requests_created, 0);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_outbox WHERE agenda_item_id = ?",
            [created.item.item_id.as_str()],
        )
        .await,
        0,
        "Agenda source scan must not bypass policy evaluation by writing legacy Outbox"
    );

    let reschedule = AgendaUseCase::new(
        build_mysql_agenda_store(db.clone()),
        Arc::new(FixedClock { now: 100_001 }),
    );
    let updated = reschedule
        .apply(&AgendaApplyRequest {
            account: account.clone(),
            command_source_event_id,
            run_id: run_id.clone(),
            effect_id: format!("agenda-reschedule-{}", Uuid::new_v4()),
            proposal_id: Uuid::new_v4().to_string(),
            proposal_json: r#"{"kind":"reschedule_item"}"#.into(),
            lease_token,
            idempotency_key: format!("agenda-reschedule-{run_id}"),
            mutation: AgendaMutation::Reschedule {
                item_id: created.item.item_id.clone(),
                expected_version: created.item.version,
                scheduled_at_unix_secs: 100_002,
                timezone: "Asia/Shanghai".into(),
            },
        })
        .await
        .unwrap();
    assert_eq!(updated.item.version, 2);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_outbox WHERE agenda_item_id = ? AND agenda_version = 1 AND delivery_status = 'suppressed'",
            [created.item.item_id.as_str()],
        )
        .await,
        0,
        "candidate-only Agenda scans must not leave legacy Outbox rows to suppress"
    );

    let rescheduled_due = AgendaUseCase::new(
        build_mysql_agenda_store(db.clone()),
        Arc::new(FixedClock { now: 100_002 }),
    );
    let rescheduled_report = rescheduled_due
        .produce_due_notification_candidates(100)
        .await
        .unwrap();
    assert_eq!(rescheduled_report.candidates_created, 1);
    assert_eq!(rescheduled_report.requests_created, 1);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_outbox WHERE agenda_item_id = ?",
            [created.item.item_id.as_str()],
        )
        .await,
        0,
        "rescheduled source scan still must not create an Outbox row before policy evaluation"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_candidates WHERE source_kind = 'agenda' AND source_id = ? AND source_version = 2",
            [created.item.item_id.as_str()],
        )
        .await,
        1
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn legacy_reconciliation_rebuilds_only_current_follow_up_sources_and_blocks_active_claims() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;

    let run_id = Uuid::new_v4().simple().to_string();
    let platform_account_id = format!("task7-reconcile-{run_id}");
    let account = SourceAccountRef::new(MessageSource::NapCat, &platform_account_id).unwrap();
    let inbound = build_mysql_inbound_event_store(db.clone());
    let source_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::NapCat, &platform_account_id, "source")
                    .unwrap(),
                ConversationRef::new(ConversationKind::Private, "owner").unwrap(),
                VerifiedActor::new(VerifiedActorKind::External, "alice").unwrap(),
                90_000,
                "我会提交材料",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let memory_store = build_mysql_memory_store(db.clone());
    let fact = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: account.clone(),
        subject_key: "commitment:task7-reconcile".into(),
        payload: MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: account.clone(),
                actor_id: "alice".into(),
            },
            beneficiary: ThreadActorRef {
                account: account.clone(),
                actor_id: "owner".into(),
            },
            action: "提交材料".into(),
            due_at_unix_secs: Some(90_001),
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_500,
        source_event_ids: vec![source_event_id],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    MemoryUseCase::new(memory_store.clone())
        .remember(&fact)
        .await
        .unwrap();
    let follow_up = personal_secretary::FollowUpUseCase::new(
        build_mysql_follow_up_store(db.clone()),
        memory_store,
    );
    follow_up
        .scan(90_001, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    let follow_up_id = scalar_string(
        &db,
        "SELECT follow_up_id AS value FROM secretary_follow_up_items WHERE source_memory_fact_id = ?",
        [fact.fact_id.as_str()],
    )
    .await
    .unwrap();

    // 删除扫描产生的链路，证明协调只能从当前受锁定来源重新建立 Candidate/Request。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE candidate FROM secretary_notification_candidates candidate WHERE candidate.source_kind = 'follow_up' AND candidate.source_id = ?",
        [follow_up_id.clone().into()],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_outbox \
         (notification_id, account_id, follow_up_id, scheduled_at_unix_secs, notification_kind, payload_json, delivery_status) \
         SELECT ?, id, ?, 90001, 'owner_reminder', JSON_OBJECT('legacy', true), 'pending' \
         FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
        [
            Uuid::new_v4().to_string().into(),
            follow_up_id.clone().into(),
            platform_account_id.clone().into(),
        ],
    ))
    .await
    .unwrap();

    let config = LegacyNotificationReconciliationConfig {
        worker_id: "task7-mysql-test".into(),
        lease_secs: 60,
        page_size: 10,
        max_rows: 10,
        deadline_secs: 10,
    };
    let report = follow_up
        .reconcile_legacy_notifications(&config)
        .await
        .unwrap();
    assert!(report.completed);
    assert!(!report.blocked);
    assert_eq!(report.rows_scanned, 1);
    assert_eq!(report.legacy_outbox_suppressed, 1);
    assert_eq!(report.legacy_sources_rebuilt, 1);
    assert_eq!(report.candidates_created, 1);
    assert_eq!(report.requests_created, 1);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_candidates WHERE source_kind = 'follow_up' AND source_id = ? AND source_version = 1",
            [&follow_up_id],
        )
        .await,
        1
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_notification_evaluation_requests request JOIN secretary_notification_candidates candidate ON candidate.notification_candidate_id = request.notification_candidate_id WHERE candidate.source_kind = 'follow_up' AND candidate.source_id = ? AND request.evaluation_generation = 1",
            [&follow_up_id],
        )
        .await,
        1
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT delivery_status AS value FROM secretary_notification_outbox WHERE follow_up_id = ?",
            [&follow_up_id],
        )
        .await
        .as_deref(),
        Some("suppressed")
    );

    let replay = follow_up
        .reconcile_legacy_notifications(&config)
        .await
        .unwrap();
    assert!(replay.completed);
    assert_eq!(replay.rows_scanned, 0);
    assert_eq!(replay.candidates_created, 0);
    assert_eq!(replay.requests_created, 0);

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_notification_outbox SET delivery_status = 'claimed', lease_token = ?, lease_expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 60 SECOND) WHERE follow_up_id = ?",
        [Uuid::new_v4().to_string().into(), follow_up_id.clone().into()],
    ))
    .await
    .unwrap();
    let blocked = follow_up
        .reconcile_legacy_notifications(&config)
        .await
        .unwrap();
    assert!(blocked.blocked);
    assert!(!blocked.completed);
    assert_eq!(blocked.active_claimed, 1);
    assert_eq!(
        scalar_string(
            &db,
            "SELECT delivery_status AS value FROM secretary_notification_outbox WHERE follow_up_id = ?",
            [&follow_up_id],
        )
        .await
        .as_deref(),
        Some("claimed"),
        "活跃租约必须保持原状并阻塞启动"
    );
}

/// 测试用固定时钟，确保 Agenda 到期扫描可重复验证。
struct FixedClock {
    now: i64,
}

impl Clock for FixedClock {
    fn now_unix_secs(&self) -> i64 {
        self.now
    }
}

async fn scalar_i64<const N: usize>(
    db: &sea_orm::DatabaseConnection,
    sql: &str,
    values: [&str; N],
) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        sql,
        values.map(Into::into),
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<i64>("", "value")
    .unwrap()
}

async fn scalar_exactly_one_string<const N: usize>(
    db: &sea_orm::DatabaseConnection,
    sql: &str,
    values: [&str; N],
    assertion: &str,
) -> String {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values.map(Into::into),
        ))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "{assertion}");
    rows[0].try_get::<String>("", "value").unwrap()
}

/// 测试 schema 的生命周期由外层脚本负责；这里复用唯一迁移加载器。
async fn apply_qqbot_migrations(db: &sea_orm::DatabaseConnection) {
    qqbot_migrations::apply_qqbot_migrations(
        db,
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/qqbot-server/database/migrations"),
    )
    .await;
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn deterministic_thread_projection_batches_reply_and_conversation_window() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("thread-account-{run_id}");

    let make_message = |message_id: &str,
                        conversation_id: &str,
                        occurred_at: i64,
                        segments: Vec<ContentSegment>| {
        InboundMessageEnvelope::new(
            SourceMessageRef::new(MessageSource::NapCat, &account_id, message_id).unwrap(),
            ConversationRef::new(ConversationKind::Group, conversation_id).unwrap(),
            VerifiedActor::new(VerifiedActorKind::External, "same-sender").unwrap(),
            occurred_at,
            "thread integration fixture",
            segments,
        )
        .unwrap()
    };

    let parent = make_message("thread-parent", "group-a", 1000, Vec::new());
    let child = make_message(
        "thread-child",
        "group-a",
        1400,
        vec![ContentSegment::Reply {
            platform_message_id: "thread-parent".into(),
        }],
    );
    let near = make_message("thread-near", "group-a", 1450, Vec::new());
    let other_group = make_message("thread-other-group", "group-b", 1451, Vec::new());
    for message in [&parent, &child, &near, &other_group] {
        inbound.insert_message_if_absent(message).await.unwrap();
    }

    let use_case = ThreadProjectionUseCase::new(
        build_mysql_thread_projection_store(db.clone()),
        DeterministicThreadPlanner::new(DeterministicThreadPolicy::new(300).unwrap()),
        100,
        60,
        300,
    )
    .unwrap();
    let run = use_case
        .run_once()
        .await
        .unwrap()
        .expect("four source events must be projected");
    // 仓储按全库消费；同一隔离 schema 中其它测试可能已留下待投影事件，因此只断言
    // 本测试的四条事件已被覆盖，精确归并结果由下方账号作用域查询验证。
    assert!(run.events_projected >= 4);
    assert!(run.threads_created >= 2);
    assert!(use_case.run_once().await.unwrap().is_none());

    let parent_thread = thread_id_for(&db, &account_id, "thread-parent").await;
    assert_eq!(
        parent_thread,
        thread_id_for(&db, &account_id, "thread-child").await
    );
    assert_eq!(
        parent_thread,
        thread_id_for(&db, &account_id, "thread-near").await
    );
    assert_ne!(
        parent_thread,
        thread_id_for(&db, &account_id, "thread-other-group").await
    );

    let reply_edges = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_thread_relations r \
         JOIN secretary_source_events e ON e.source_event_id = r.from_event_id \
         JOIN secretary_accounts a ON a.id = e.account_id \
         WHERE a.platform_account_id = ? AND r.relation_kind = 'reply'",
        [&account_id],
    )
    .await;
    assert_eq!(reply_edges, 1);
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn thread_semantics_persist_typed_candidates_sources_and_privacy_filter() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("semantic-account-{run_id}");

    let texts = [
        ("semantic-request", "请发送报价单"),
        ("semantic-objection", "我反对周一上线"),
        ("semantic-confirmation", "确认：采用第二版"),
        ("semantic-decision", "决定：周五发布"),
        ("semantic-question", "什么时候回复客户？"),
    ];
    for (index, (message_id, text)) in texts.iter().enumerate() {
        let envelope = InboundMessageEnvelope::new(
            SourceMessageRef::new(MessageSource::NapCat, &account_id, *message_id).unwrap(),
            ConversationRef::new(ConversationKind::Group, "semantic-group").unwrap(),
            VerifiedActor::new(VerifiedActorKind::External, format!("actor-{index}")).unwrap(),
            2000 + index as i64,
            *text,
            Vec::new(),
        )
        .unwrap();
        inbound.insert_message_if_absent(&envelope).await.unwrap();
    }
    let private = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &account_id, "semantic-private").unwrap(),
        ConversationRef::new(ConversationKind::Private, "private-never").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "private-actor").unwrap(),
        3000,
        "请保存这条隐私消息",
        Vec::new(),
    )
    .unwrap();
    inbound.insert_message_if_absent(&private).await.unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_conversations c JOIN secretary_accounts a ON a.id = c.account_id \
         SET c.memory_mode = 'never_long_term' \
         WHERE a.platform_account_id = ? AND c.platform_conversation_id = 'private-never'",
        [account_id.clone().into()],
    ))
    .await
    .unwrap();

    let projection = ThreadProjectionUseCase::new(
        build_mysql_thread_projection_store(db.clone()),
        DeterministicThreadPlanner::new(DeterministicThreadPolicy::new(300).unwrap()),
        100,
        60,
        300,
    )
    .unwrap();
    while projection.run_once().await.unwrap().is_some() {}

    let semantics = ThreadSemanticUseCase::new(
        build_mysql_thread_semantic_store(db.clone()),
        Arc::new(ConservativeThreadSemanticExtractor::new(10_000).unwrap()),
        50,
        50_000,
        60,
    )
    .unwrap();
    let mut processed = 0usize;
    while let Some(run) = semantics.run_once().await.unwrap() {
        processed += run.events_read;
    }
    assert!(processed >= 5);

    let claim_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_thread_claims tc \
         JOIN secretary_event_threads t ON t.thread_id = tc.thread_id \
         JOIN secretary_accounts a ON a.id = t.account_id \
         WHERE a.platform_account_id = ?",
        [&account_id],
    )
    .await;
    let decision_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_thread_decisions td \
         JOIN secretary_event_threads t ON t.thread_id = td.thread_id \
         JOIN secretary_accounts a ON a.id = t.account_id \
         WHERE a.platform_account_id = ?",
        [&account_id],
    )
    .await;
    let question_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_thread_open_questions tq \
         JOIN secretary_event_threads t ON t.thread_id = tq.thread_id \
         JOIN secretary_accounts a ON a.id = t.account_id \
         WHERE a.platform_account_id = ?",
        [&account_id],
    )
    .await;
    assert_eq!(claim_count, 3);
    assert_eq!(decision_count, 1);
    assert_eq!(question_count, 1);

    let source_count = scalar_i64(
        &db,
        "SELECT (\
             (SELECT COUNT(*) FROM secretary_thread_claim_sources cs \
              JOIN secretary_source_events e ON e.source_event_id = cs.source_event_id \
              JOIN secretary_accounts a ON a.id = e.account_id WHERE a.platform_account_id = ?) +\
             (SELECT COUNT(*) FROM secretary_thread_decision_sources ds \
              JOIN secretary_source_events e ON e.source_event_id = ds.source_event_id \
              JOIN secretary_accounts a ON a.id = e.account_id WHERE a.platform_account_id = ?) +\
             (SELECT COUNT(*) FROM secretary_thread_question_sources qs \
              JOIN secretary_source_events e ON e.source_event_id = qs.source_event_id \
              JOIN secretary_accounts a ON a.id = e.account_id WHERE a.platform_account_id = ?)\
         ) AS value",
        [&account_id, &account_id, &account_id],
    )
    .await;
    assert_eq!(source_count, 5);

    let private_candidates = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_thread_claim_sources cs \
         JOIN secretary_source_events e ON e.source_event_id = cs.source_event_id \
         JOIN secretary_accounts a ON a.id = e.account_id \
         WHERE a.platform_account_id = ? AND e.platform_event_id = 'semantic-private'",
        [&account_id],
    )
    .await;
    assert_eq!(private_candidates, 0);
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn cross_conversation_links_are_proposed_from_strong_evidence_only() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("thread-link-account-{run_id}");

    let fixtures = [
        (
            "link-project-group",
            ConversationKind::Group,
            "link-group",
            "项目ID:PAYMENT_V2",
            Vec::new(),
        ),
        (
            "link-project-private",
            ConversationKind::Private,
            "link-private",
            "项目ID:PAYMENT_V2",
            Vec::new(),
        ),
        (
            "link-file-a",
            ConversationKind::Group,
            "file-group-a",
            "请看附件",
            vec![ContentSegment::Media {
                kind: personal_secretary::MediaKind::File,
                source_key: "content-key-a".into(),
                source_url: None,
                display_name: Some("报价单.pdf".into()),
            }],
        ),
        (
            "link-file-b",
            ConversationKind::Group,
            "file-group-b",
            "请看附件",
            vec![ContentSegment::Media {
                kind: personal_secretary::MediaKind::File,
                source_key: "content-key-b".into(),
                source_url: None,
                display_name: Some("报价单.pdf".into()),
            }],
        ),
        (
            "link-private-excluded",
            ConversationKind::Private,
            "link-never-long-term",
            "项目ID:PAYMENT_V2",
            Vec::new(),
        ),
    ];
    for (index, (message_id, kind, conversation_id, text, segments)) in
        fixtures.into_iter().enumerate()
    {
        let envelope = InboundMessageEnvelope::new(
            SourceMessageRef::new(MessageSource::NapCat, &account_id, message_id).unwrap(),
            ConversationRef::new(kind, conversation_id).unwrap(),
            VerifiedActor::new(VerifiedActorKind::External, "same-person").unwrap(),
            10_000 + index as i64 * 1000,
            text,
            segments,
        )
        .unwrap();
        inbound.insert_message_if_absent(&envelope).await.unwrap();
    }
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_conversations c JOIN secretary_accounts a ON a.id = c.account_id \
         SET c.memory_mode = 'never_long_term' \
         WHERE a.platform_account_id = ? AND c.platform_conversation_id = 'link-never-long-term'",
        [account_id.clone().into()],
    ))
    .await
    .unwrap();

    let projection = ThreadProjectionUseCase::new(
        build_mysql_thread_projection_store(db.clone()),
        DeterministicThreadPlanner::new(DeterministicThreadPolicy::new(300).unwrap()),
        100,
        60,
        300,
    )
    .unwrap();
    while projection.run_once().await.unwrap().is_some() {}

    let links = ThreadLinkUseCase::new(build_mysql_thread_link_store(db.clone()), 100, 100_000, 60)
        .unwrap();
    while links.run_once().await.unwrap().is_some() {}

    let candidate_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_thread_link_candidates candidate \
         JOIN secretary_accounts account ON account.id = candidate.account_id \
         WHERE account.platform_account_id = ?",
        [&account_id],
    )
    .await;
    assert_eq!(
        candidate_count, 1,
        "same actor/topic/filename and excluded conversations must not create candidates"
    );
    let proposed_project_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_thread_link_candidates candidate \
         JOIN secretary_accounts account ON account.id = candidate.account_id \
         WHERE account.platform_account_id = ? AND candidate.status = 'proposed' \
         AND candidate.signal_kind = 'explicit_project_id' \
         AND candidate.reason_code = 'explicit_project_id' AND candidate.confidence_bps = 9500",
        [&account_id],
    )
    .await;
    assert_eq!(proposed_project_count, 1);
    let source_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_thread_link_candidate_sources source \
         JOIN secretary_thread_link_candidates candidate ON candidate.candidate_id = source.candidate_id \
         JOIN secretary_accounts account ON account.id = candidate.account_id \
         WHERE account.platform_account_id = ?",
        [&account_id],
    )
    .await;
    assert_eq!(source_count, 2, "both conversations must remain auditable");
    let raw_hint_leaks = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_thread_link_hints hint \
         JOIN secretary_accounts account ON account.id = hint.account_id \
         WHERE account.platform_account_id = ? AND hint.fingerprint_sha256 = 'payment_v2'",
        [&account_id],
    )
    .await;
    assert_eq!(
        raw_hint_leaks, 0,
        "raw project ids must not be copied into hint storage"
    );

    let candidate_id = ThreadLinkCandidateId::new(
        scalar_string(
            &db,
            "SELECT candidate.candidate_id AS value \
             FROM secretary_thread_link_candidates candidate \
             JOIN secretary_accounts account ON account.id = candidate.account_id \
             WHERE account.platform_account_id = ?",
            [&account_id],
        )
        .await
        .expect("candidate must exist"),
    )
    .unwrap();
    let review = ThreadLinkReviewUseCase::new(build_mysql_thread_link_store(db.clone()));
    let account = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();
    let inbox = review.list(&account, None, 10).await.unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].sources.len(), 2);
    assert!(
        inbox[0]
            .sources
            .iter()
            .all(|source| !source.excerpt.is_empty())
    );
    assert!(review.list(&account, None, 0).await.is_err());

    let non_command = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &account_id, "link-not-owner-command")
            .unwrap(),
        ConversationRef::new(ConversationKind::Private, "ordinary-private").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "external").unwrap(),
        20_000,
        "接受关联",
        Vec::new(),
    )
    .unwrap();
    let non_command_id = inbound
        .insert_message_if_absent(&non_command)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    assert!(
        review
            .review(
                &candidate_id,
                &non_command_id,
                ThreadLinkReviewAction::Accept
            )
            .await
            .is_err(),
        "ordinary observations must not review candidates"
    );

    let other_account_id = format!("thread-link-other-control-{run_id}");
    let other_owner_command = InboundMessageEnvelope::new(
        SourceMessageRef::new(
            MessageSource::QqOpenPlatform,
            &other_account_id,
            "link-other-owner-command",
        )
        .unwrap(),
        ConversationRef::new(ConversationKind::OwnerControl, "owner-control").unwrap(),
        VerifiedActor::new(VerifiedActorKind::Owner, "owner").unwrap(),
        20_001,
        "接受关联",
        Vec::new(),
    )
    .unwrap();
    let other_command_id = inbound
        .insert_message_if_absent(&other_owner_command)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    assert!(
        review
            .review(
                &candidate_id,
                &other_command_id,
                ThreadLinkReviewAction::Accept
            )
            .await
            .is_err(),
        "an OwnerCommand from another account must not cross the account boundary"
    );

    let command_account_id = format!("thread-link-owner-control-{run_id}");
    let owner_command = InboundMessageEnvelope::new(
        SourceMessageRef::new(
            MessageSource::QqOpenPlatform,
            &command_account_id,
            "link-owner-command",
        )
        .unwrap(),
        ConversationRef::new(ConversationKind::OwnerControl, "owner-control").unwrap(),
        VerifiedActor::new(VerifiedActorKind::Owner, "owner").unwrap(),
        20_002,
        "接受关联",
        Vec::new(),
    )
    .unwrap();
    let command_id = inbound
        .insert_message_if_absent(&owner_command)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_owner_bindings \
         (binding_id, managed_account_id, command_account_id, owner_actor_id, status) \
         SELECT ?, managed.id, command.id, 'owner', 'active' \
         FROM secretary_accounts managed JOIN secretary_accounts command \
         WHERE managed.source_channel = 'napcat' AND managed.platform_account_id = ? \
         AND command.source_channel = 'qq_open_platform' \
         AND command.platform_account_id = ?",
        [
            Uuid::new_v4().to_string().into(),
            account_id.clone().into(),
            command_account_id.into(),
        ],
    ))
    .await
    .unwrap();
    let first = review
        .review(&candidate_id, &command_id, ThreadLinkReviewAction::Accept)
        .await
        .unwrap();
    assert!(first.changed);
    let repeated = review
        .review(&candidate_id, &command_id, ThreadLinkReviewAction::Accept)
        .await
        .unwrap();
    assert!(!repeated.changed);
    assert_eq!(first.review_id, repeated.review_id);
    let review_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_thread_link_reviews review \
         JOIN secretary_thread_link_candidates candidate ON candidate.candidate_id = review.candidate_id \
         JOIN secretary_accounts account ON account.id = candidate.account_id \
         WHERE account.platform_account_id = ? AND candidate.status = 'accepted' \
         AND review.review_action = 'accept'",
        [&account_id],
    )
    .await;
    assert_eq!(review_count, 1, "repeated review must remain idempotent");
    let project_thread_count = scalar_i64(
        &db,
        "SELECT COUNT(DISTINCT te.thread_id) AS value FROM secretary_thread_events te \
         JOIN secretary_source_events event ON event.source_event_id = te.source_event_id \
         JOIN secretary_accounts account ON account.id = event.account_id \
         WHERE account.platform_account_id = ? \
         AND event.platform_event_id IN ('link-project-group', 'link-project-private')",
        [&account_id],
    )
    .await;
    assert_eq!(
        project_thread_count, 2,
        "accepting a candidate must not automatically merge thread membership"
    );
}

/// 建立一个空窗已结束的 uncertain Gap：连接→上线→断开（创建开放空窗）→重连（结束空窗）。
/// 回补仅领取 `gap_ended_at IS NOT NULL` 的 Gap，避免回补尚未结束的离线窗口。
async fn create_uncertain_gap(
    store: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    account_id: &str,
) -> personal_secretary::IngestionGapId {
    let account = SourceAccountRef::new(MessageSource::NapCat, account_id).unwrap();
    let epoch = store.begin_connection(&account).await.unwrap();
    store.mark_connection_connected(&epoch).await.unwrap();
    let gap = store
        .finish_connection(&epoch, ConnectionEndReason::TransportError)
        .await
        .unwrap()
        .expect("connected epoch must create an uncertain gap");
    // 重连：mark_connection_connected 会结束该账号所有开放空窗（gap_ended_at 置位）。
    let epoch2 = store.begin_connection(&account).await.unwrap();
    store.mark_connection_connected(&epoch2).await.unwrap();
    gap
}
#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn backfill_migrations_apply_in_order_and_are_idempotent() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    // 重复执行迁移必须安全（CREATE TABLE IF NOT EXISTS / upsert）。
    apply_qqbot_migrations(&db).await;

    // 26 张 secretary_* 表（入站 4 + 连续性 4 + 回补 5 + 线程投影 4 + 语义 9）必须存在。
    let table_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name LIKE 'secretary_%'",
        [""; 0],
    )
    .await;
    assert!(
        table_count >= 26,
        "expected at least 26 secretary_* tables, got {table_count}"
    );

    let backfill_run_table = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = 'secretary_backfill_runs'",
        [""; 0],
    )
    .await;
    assert_eq!(backfill_run_table, 1);
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn backfill_claim_is_atomic_and_lease_recovery_works() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let continuity_store = build_mysql_inbound_event_store(db.clone());
    let backfill_store = build_mysql_backfill_store(db.clone(), 60);
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("backfill-claim-{run_id}");
    let gap_id = create_uncertain_gap(&continuity_store, &account_id).await;

    // 第一次领取成功。
    let lease = BackfillLease::new(60);
    let claimed = backfill_store.claim_next_gap(lease).await.unwrap();
    let claimed = claimed.expect("a gap must be claimable");
    assert_eq!(claimed.gap_id, gap_id);
    assert!(!claimed.is_resume);

    // Gap 必须处于 backfilling。
    let status = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_ingestion_gaps \
         WHERE gap_id = ? AND status = 'backfilling'",
        [gap_id.as_str()],
    )
    .await;
    assert_eq!(status, 1);

    // 第二次领取无 Gap 可领取。
    let second = backfill_store.claim_next_gap(lease).await.unwrap();
    assert!(second.is_none(), "the same gap must not be claimed twice");

    // 模拟租约过期：把 lease_expires_at 置为过去，reclaim_expired 必须恢复该运行。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_backfill_runs SET lease_expires_at = '2020-01-01 00:00:00' \
         WHERE backfill_run_id = ?",
        [claimed.run_id.as_str()].map(Into::into),
    ))
    .await
    .unwrap();
    // 两个恢复者并发竞争时，FOR UPDATE + CAS 只能让一个拿到该运行。
    let competing_store = Arc::clone(&backfill_store);
    let (first_reclaim, second_reclaim) = tokio::join!(
        backfill_store.reclaim_expired(lease, 10),
        competing_store.reclaim_expired(lease, 10)
    );
    let mut reclaimed = first_reclaim.unwrap();
    reclaimed.extend(second_reclaim.unwrap());
    assert_eq!(reclaimed.len(), 1);
    assert!(reclaimed[0].is_resume);
    assert_eq!(reclaimed[0].run_id, claimed.run_id);
    assert_ne!(
        reclaimed[0].lease_token, claimed.lease_token,
        "lease recovery must rotate the fencing token"
    );

    // 旧 Worker 即使稍后恢复，也不能用过期令牌提交 Gap 终态。
    let outcome = BackfillOutcome {
        run_id: claimed.run_id.clone(),
        gap_id: gap_id.clone(),
        completeness: HistoryCompleteness::Unprovable,
        evidence: BackfillEvidence::default(),
        gap_target_status: IngestionGapStatus::Uncertain,
        gap_reason: Some(IngestionGapReason::HistoryUnprovable),
        failure_class: None,
    };
    let stale_finalize = backfill_store
        .finalize_run(&outcome, &claimed.lease_token)
        .await;
    assert!(matches!(
        stale_finalize,
        Err(personal_secretary::InboundEventStoreError::LeaseLost)
    ));
    let still_backfilling = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_ingestion_gaps \
         WHERE gap_id = ? AND status = 'backfilling'",
        [gap_id.as_str()],
    )
    .await;
    assert_eq!(still_backfilling, 1);

    // 即使令牌正确，也不能把一个运行的结果提交到其它 Gap。
    let mut wrong_gap_outcome = outcome.clone();
    wrong_gap_outcome.gap_id = personal_secretary::IngestionGapId::new("wrong-gap").unwrap();
    let wrong_gap_finalize = backfill_store
        .finalize_run(&wrong_gap_outcome, &reclaimed[0].lease_token)
        .await;
    assert!(matches!(
        wrong_gap_finalize,
        Err(personal_secretary::InboundEventStoreError::InvalidData(_))
    ));

    // 当前持有者的令牌仍可正常提交。
    backfill_store
        .finalize_run(&outcome, &reclaimed[0].lease_token)
        .await
        .unwrap();
    backfill_store
        .finalize_run(&outcome, &reclaimed[0].lease_token)
        .await
        .expect("retrying the same committed finalize must be idempotent");
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn backfill_realtime_and_history_share_one_idempotent_entry() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let store = build_mysql_backfill_store(db.clone(), 60);
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("backfill-idem-{run_id}");

    // 实时先到：消息 M 落库。
    let realtime = message(&account_id, "M", Vec::new());
    let accepted = store.insert_message_if_absent(&realtime).await.unwrap();
    assert!(matches!(accepted, IngestMessageOutcome::Accepted { .. }));

    // 历史后到：同一条 M 经统一幂等入口返回 Duplicate，不产生新 SourceEvent。
    let duplicate = store.insert_message_if_absent(&realtime).await.unwrap();
    assert!(matches!(duplicate, IngestMessageOutcome::Duplicate { .. }));

    let count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value \
         FROM secretary_source_events event \
         INNER JOIN secretary_accounts account ON account.id = event.account_id \
         WHERE account.platform_account_id = ? AND event.platform_event_id = 'M'",
        [&account_id],
    )
    .await;
    assert_eq!(count, 1, "history must not duplicate the realtime event");
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn backfill_reply_child_before_parent_is_backfilled_within_account() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let store = build_mysql_backfill_store(db.clone(), 60);
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("reply-backfill-{run_id}");

    // 子消息先到：引用父平台消息 ID "parent-1"，但父消息尚未入库，reply_to_event_id 为空。
    let child = message(
        &account_id,
        "child-1",
        vec![ContentSegment::Reply {
            platform_message_id: "parent-1".into(),
        }],
    );
    let child_outcome = store.insert_message_if_absent(&child).await.unwrap();
    let child_id = match child_outcome {
        IngestMessageOutcome::Accepted {
            source_event_id,
            reply_to_event_id,
        } => {
            assert!(
                reply_to_event_id.is_none(),
                "child must be unresolved before parent arrives"
            );
            source_event_id
        }
        _ => panic!("expected child accepted"),
    };

    // 父消息后到：触发同账号内 reply_to_event_id 回填。
    let parent = message(&account_id, "parent-1", Vec::new());
    let parent_outcome = store.insert_message_if_absent(&parent).await.unwrap();
    let parent_id = match parent_outcome {
        IngestMessageOutcome::Accepted {
            source_event_id, ..
        } => source_event_id,
        _ => panic!("expected parent accepted"),
    };

    // 子消息的 reply_to_event_id 必须已被回填为父事件 ID。
    let child_reply = scalar_string(
        &db,
        "SELECT reply_to_event_id AS value FROM secretary_source_events WHERE source_event_id = ?",
        [child_id.as_str()],
    )
    .await;
    assert_eq!(child_reply, Some(parent_id.as_str().to_owned()));

    // 跨账号：另一账号引用同一父平台 ID 不得被回填。
    let other_account = format!("reply-other-{run_id}");
    let other_child = message(
        &other_account,
        "other-child",
        vec![ContentSegment::Reply {
            platform_message_id: "parent-1".into(),
        }],
    );
    let other_outcome = store.insert_message_if_absent(&other_child).await.unwrap();
    let other_id = other_outcome.source_event_id().as_str().to_owned();
    let other_reply = scalar_string(
        &db,
        "SELECT reply_to_event_id AS value FROM secretary_source_events WHERE source_event_id = ?",
        [other_id.as_str()],
    )
    .await;
    assert_eq!(
        other_reply, None,
        "reply must not bind across account subjects"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn backfill_cross_account_isolation_holds() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let store = build_mysql_backfill_store(db.clone(), 60);
    let run_id = Uuid::new_v4().simple().to_string();
    let account_a = format!("iso-a-{run_id}");
    let account_b = format!("iso-b-{run_id}");

    // 两个账号使用同一平台消息 ID 必须各自独立落库。
    let msg_a = message(&account_a, "shared-1", Vec::new());
    let msg_b = message(&account_b, "shared-1", Vec::new());
    assert!(matches!(
        store.insert_message_if_absent(&msg_a).await.unwrap(),
        IngestMessageOutcome::Accepted { .. }
    ));
    assert!(matches!(
        store.insert_message_if_absent(&msg_b).await.unwrap(),
        IngestMessageOutcome::Accepted { .. }
    ));

    let count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value \
         FROM secretary_source_events event \
         INNER JOIN secretary_accounts account ON account.id = event.account_id \
         WHERE account.platform_account_id IN (?, ?) AND event.platform_event_id = 'shared-1'",
        [&account_a, &account_b],
    )
    .await;
    assert_eq!(
        count, 2,
        "cross-account same platform id must be kept separate"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn backfill_use_case_with_fake_source_marks_gap_complete_via_mysql() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let continuity_store = build_mysql_inbound_event_store(db.clone());
    let backfill_store = build_mysql_backfill_store(db.clone(), 60);
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("usecase-{run_id}");

    // 实时落库边界消息 + 建立会话游标，使该会话成为已知 Scope。
    let epoch = continuity_store
        .begin_connection(&SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap())
        .await
        .unwrap();
    continuity_store
        .mark_connection_connected(&epoch)
        .await
        .unwrap();
    let boundary = message(&account_id, "boundary-1", Vec::new()).observed_in(epoch.clone());
    continuity_store
        .insert_message_if_absent(&boundary)
        .await
        .unwrap();
    let gap_id = continuity_store
        .finish_connection(&epoch, ConnectionEndReason::TransportError)
        .await
        .unwrap()
        .unwrap();
    // 重连结束空窗：回补仅领取 gap_ended_at IS NOT NULL 的 Gap。
    let epoch2 = continuity_store
        .begin_connection(&SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap())
        .await
        .unwrap();
    continuity_store
        .mark_connection_connected(&epoch2)
        .await
        .unwrap();

    // 确定性 Fake 来源：提供充分证据（账号会话集合可证完整 + 回读到边界快照）。
    // 边界快照在 finish_connection 时冻结为 "boundary-1"；FakeSource 返回同 ID 的历史消息，
    // 用例按 message_id 匹配命中边界 => reached_boundary => ProvenComplete => verified_complete。
    // 这是验证修复“边界永不命中”的成功路径测试（修复前只能落到 uncertain）。
    let account = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();
    let fake = Arc::new(FakeHistorySource::new(account.clone(), true, "boundary-1"));
    let budget = BackfillBudget {
        page_size: 10,
        max_pages_per_scope: 5,
        max_events_per_run: 100,
        max_concurrency: 1,
        lease_secs: 60,
        retry_initial_ms: 1,
        retry_max_ms: 2,
    };
    let use_case = BackfillGapUseCase::new(backfill_store.clone(), fake, budget);

    let outcome = use_case
        .run_one()
        .await
        .unwrap()
        .expect("gap must be processed");
    assert_eq!(outcome.gap_id, gap_id);
    // 充分证据 + 账号会话集合可证完整 => verified_complete（成功路径可达）。
    assert_eq!(outcome.completeness, HistoryCompleteness::ProvenComplete);
    assert_eq!(
        outcome.gap_target_status,
        IngestionGapStatus::VerifiedComplete
    );

    let gap_status = scalar_string(
        &db,
        "SELECT status AS value FROM secretary_ingestion_gaps WHERE gap_id = ?",
        [gap_id.as_str()],
    )
    .await;
    assert_eq!(gap_status.as_deref(), Some("verified_complete"));
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn backfill_open_gap_is_not_claimable_until_window_ends() {
    // 修复 #1：空窗未结束（gap_ended_at IS NULL）时不可领取，避免回补尚未结束的离线窗口。
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let continuity_store = build_mysql_inbound_event_store(db.clone());
    let backfill_store = build_mysql_backfill_store(db.clone(), 60);
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("open-gap-{run_id}");
    let account = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();

    let epoch = continuity_store.begin_connection(&account).await.unwrap();
    continuity_store
        .mark_connection_connected(&epoch)
        .await
        .unwrap();
    let gap_id = continuity_store
        .finish_connection(&epoch, ConnectionEndReason::TransportError)
        .await
        .unwrap()
        .unwrap();

    // 空窗尚未结束：不可领取。
    let claim = backfill_store
        .claim_next_gap(BackfillLease::new(60))
        .await
        .unwrap();
    assert!(
        claim.is_none(),
        "open gap (gap_ended_at NULL) must not be claimable"
    );

    // 重连结束空窗后：可领取。
    let epoch2 = continuity_store.begin_connection(&account).await.unwrap();
    continuity_store
        .mark_connection_connected(&epoch2)
        .await
        .unwrap();
    let claim = backfill_store
        .claim_next_gap(BackfillLease::new(60))
        .await
        .unwrap()
        .expect("ended gap must be claimable");
    assert_eq!(claim.gap_id, gap_id);
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn backfill_uncertain_gap_can_be_reclaimed_after_unprovable() {
    // 修复 #2：证据不足回到 uncertain 的 Gap 可再次回补（运行表 gap_id 无唯一键），
    // 且受 next_eligible_at 退避约束（防热循环与饿死）。
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let continuity_store = build_mysql_inbound_event_store(db.clone());
    let backfill_store = build_mysql_backfill_store(db.clone(), 60);
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("reclaim-{run_id}");
    let gap_id = create_uncertain_gap(&continuity_store, &account_id).await;
    let lease = BackfillLease::new(60);

    // 第一次领取 -> run1。
    let claimed1 = backfill_store
        .claim_next_gap(lease)
        .await
        .unwrap()
        .expect("first claim must succeed");

    // 以 Unprovable 终结：Gap 回到 uncertain，并设置 next_eligible_at = now + 30s 退避。
    let outcome = BackfillOutcome {
        run_id: claimed1.run_id.clone(),
        gap_id: gap_id.clone(),
        completeness: HistoryCompleteness::Unprovable,
        evidence: BackfillEvidence::default(),
        gap_target_status: IngestionGapStatus::Uncertain,
        gap_reason: Some(IngestionGapReason::HistoryUnprovable),
        failure_class: None,
    };
    backfill_store
        .finalize_run(&outcome, &claimed1.lease_token)
        .await
        .unwrap();

    // Gap 回到 uncertain 但处于退避期：立即领取返回 None。
    let immediate = backfill_store.claim_next_gap(lease).await.unwrap();
    assert!(
        immediate.is_none(),
        "gap must respect reclaim backoff (next_eligible_at in the future)"
    );

    // 模拟退避已过：把 next_eligible_at 置为过去。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_gap_reclaim_schedule SET next_eligible_at = '2020-01-01 00:00:00' \
         WHERE gap_id = ?",
        [gap_id.as_str()].map(Into::into),
    ))
    .await
    .unwrap();

    // 再次领取 -> 新运行 run2（与 run1 不同），证明无唯一键错误、可二次回补。
    let claimed2 = backfill_store
        .claim_next_gap(lease)
        .await
        .unwrap()
        .expect("gap must be reclaimable after backoff");
    assert_ne!(
        claimed2.run_id, claimed1.run_id,
        "re-claim must create a new run (no unique key on gap_id)"
    );
    assert_eq!(claimed2.gap_id, gap_id);

    // 两条运行记录共存，证明一个 Gap 可有多条历史运行。
    let run_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_backfill_runs WHERE gap_id = ?",
        [gap_id.as_str()],
    )
    .await;
    assert_eq!(run_count, 2, "a gap may have multiple backfill runs");
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn backfill_known_scopes_complete_suspends_auto_retry() {
    // P2 修复：KnownScopesComplete 表示所有已知 Scope 已回补完成，但账号会话集合不可证。
    // 由于 Gap 边界在创建时已冻结，重跑不会获得新证据。因此 finalize_run 应设置
    // 极远未来的 next_eligible_at（ReclaimPolicy::Suspended），使该 Gap 在自动领取查询中
    // 永远不可领取，停止重复回补。仅人工重验（删除/更新 reclaim_schedule 行）后才重新排队。
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let continuity_store = build_mysql_inbound_event_store(db.clone());
    let backfill_store = build_mysql_backfill_store(db.clone(), 60);
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("known-scopes-{run_id}");
    let gap_id = create_uncertain_gap(&continuity_store, &account_id).await;
    let lease = BackfillLease::new(60);

    // 领取 Gap。
    let claimed = backfill_store
        .claim_next_gap(lease)
        .await
        .unwrap()
        .expect("first claim must succeed");

    // 以 KnownScopesComplete 终结：Gap 回到 uncertain，reclaim_policy 为 Suspended，
    // finalize_run 应设置极远未来的 next_eligible_at。
    let outcome = BackfillOutcome {
        run_id: claimed.run_id.clone(),
        gap_id: gap_id.clone(),
        completeness: HistoryCompleteness::KnownScopesComplete,
        evidence: BackfillEvidence::default(),
        gap_target_status: IngestionGapStatus::Uncertain,
        gap_reason: Some(IngestionGapReason::HistoryUnprovable),
        failure_class: None,
    };
    backfill_store
        .finalize_run(&outcome, &claimed.lease_token)
        .await
        .unwrap();

    // reclaim_schedule 行应存在，且 next_eligible_at 在极远未来。
    let schedule_count = scalar_i64(
        &db,
        "SELECT COUNT(*) AS value FROM secretary_gap_reclaim_schedule WHERE gap_id = ?",
        [gap_id.as_str()],
    )
    .await;
    assert_eq!(
        schedule_count, 1,
        "KnownScopesComplete must keep reclaim schedule row with suspended next_eligible_at"
    );

    // next_eligible_at 应在 9999 年（Suspended 策略）。
    let eligible_year = scalar_i64(
        &db,
        "SELECT CAST(YEAR(next_eligible_at) AS SIGNED) AS value \
         FROM secretary_gap_reclaim_schedule \
         WHERE gap_id = ?",
        [gap_id.as_str()],
    )
    .await;
    assert_eq!(
        eligible_year, 9999,
        "KnownScopesComplete must suspend via far-future next_eligible_at"
    );

    // 立即领取应返回 None：Gap 虽为 uncertain，但 next_eligible_at 在极远未来。
    let immediate = backfill_store.claim_next_gap(lease).await.unwrap();
    assert!(
        immediate.is_none(),
        "KnownScopesComplete gap must not be auto-reclaimed (suspended)"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn backfill_record_progress_uses_configured_lease_seconds() {
    // 修复 #6b：record_scope_progress 必须使用配置的 lease_secs 续租，而非写死 60s。
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let continuity_store = build_mysql_inbound_event_store(db.clone());
    let backfill_store = build_mysql_backfill_store(db.clone(), 42); // lease_secs = 42
    let run_id = Uuid::new_v4().simple().to_string();
    let account_id = format!("lease-{run_id}");
    let account = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();

    // 落库一条消息建立会话游标，使该会话成为已知 Scope。
    let epoch = continuity_store.begin_connection(&account).await.unwrap();
    continuity_store
        .mark_connection_connected(&epoch)
        .await
        .unwrap();
    let msg = message(&account_id, "lease-msg", Vec::new()).observed_in(epoch.clone());
    continuity_store
        .insert_message_if_absent(&msg)
        .await
        .unwrap();
    continuity_store
        .finish_connection(&epoch, ConnectionEndReason::TransportError)
        .await
        .unwrap();
    let epoch2 = continuity_store.begin_connection(&account).await.unwrap();
    continuity_store
        .mark_connection_connected(&epoch2)
        .await
        .unwrap();

    let claimed = backfill_store
        .claim_next_gap(BackfillLease::new(60))
        .await
        .unwrap()
        .expect("gap must be claimable");

    let progress = ScopeProgress {
        conversation: ConversationRef::new(ConversationKind::Group, "group-1").unwrap(),
        status: BackfillScopeStatus::Backfilling,
        last_cursor: Some(BackfillCursor::new(
            account.clone(),
            BackfillAnchor::new("lease-msg".to_string(), String::new()),
        )),
        pages_read: 1,
        events_read: 1,
        accepted: 1,
        duplicates: 0,
        reached_boundary: false,
        anomalies: Vec::new(),
    };
    backfill_store
        .record_scope_progress(&claimed.run_id, &claimed.lease_token, &progress)
        .await
        .unwrap();

    // lease_expires_at - updated_at 应等于配置的 42s，而非写死的 60s。
    let lease_secs = scalar_i64(
        &db,
        "SELECT TIMESTAMPDIFF(SECOND, updated_at, lease_expires_at) AS value \
         FROM secretary_backfill_runs WHERE backfill_run_id = ?",
        [claimed.run_id.as_str()],
    )
    .await;
    assert_eq!(
        lease_secs, 42,
        "record_scope_progress must use configured lease_secs (42), not hardcoded 60"
    );
}

/// 确定性 Fake 历史来源：返回一页包含边界消息的历史，并声明账号会话集合可证完整。
struct FakeHistorySource {
    account: SourceAccountRef,
    proven: bool,
    boundary_message_id: String,
    fetched: std::sync::atomic::AtomicUsize,
}

impl FakeHistorySource {
    fn new(account: SourceAccountRef, proven: bool, boundary_message_id: &str) -> Self {
        Self {
            account,
            proven,
            boundary_message_id: boundary_message_id.to_string(),
            fetched: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl HistoryBackfillSourceT for FakeHistorySource {
    async fn fetch_page(
        &self,
        scope: &personal_secretary::BackfillScope,
        cursor: Option<&BackfillCursor>,
        _page_size: u32,
    ) -> Result<personal_secretary::BackfillPage, personal_secretary::BackfillSourceError> {
        let already = self
            .fetched
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if already > 0 || cursor.is_some() {
            // 第二次调用返回空页到达起点。
            return Ok(personal_secretary::BackfillPage {
                items: Vec::new(),
                next_cursor: None,
            });
        }
        let envelope = InboundMessageEnvelope::new(
            SourceMessageRef::new(
                MessageSource::NapCat,
                self.account.account_id.clone(),
                self.boundary_message_id.clone(),
            )
            .unwrap(),
            scope.conversation.clone(),
            VerifiedActor::new(VerifiedActorKind::External, "sender-1").unwrap(),
            1_800_000_000,
            "",
            Vec::new(),
        )
        .unwrap();
        let anchor = BackfillAnchor::new(self.boundary_message_id.clone(), "seed");
        Ok(personal_secretary::BackfillPage {
            items: vec![personal_secretary::BackfillHistoryItem { envelope, anchor }],
            next_cursor: None,
        })
    }

    fn account_conversation_set_proven(&self) -> bool {
        self.proven
    }
}

async fn scalar_string<const N: usize>(
    db: &sea_orm::DatabaseConnection,
    sql: &str,
    values: [&str; N],
) -> Option<String> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values.map(Into::into),
        ))
        .await
        .unwrap()
        .unwrap();
    let value: Option<String> = row.try_get("", "value").unwrap();
    value.filter(|v| !v.is_empty())
}

async fn thread_id_for(
    db: &sea_orm::DatabaseConnection,
    account_id: &str,
    platform_event_id: &str,
) -> String {
    scalar_string(
        db,
        "SELECT te.thread_id AS value FROM secretary_thread_events te \
         JOIN secretary_source_events e ON e.source_event_id = te.source_event_id \
         JOIN secretary_accounts a ON a.id = e.account_id \
         WHERE a.platform_account_id = ? AND e.platform_event_id = ?",
        [account_id, platform_event_id],
    )
    .await
    .expect("event must have a thread")
}
