use agent_core::AgentState;
use agent_core::graph::{
    GraphDefinition, GraphExecutionResult, GraphId, GraphPolicy, GraphRuntime, NodeId, RunBudget,
    TransitionRule,
};
use async_trait::async_trait;
use personal_secretary::{
    ActionLeaseToken, ActionPlannerT, ActionRunId, ActionRunSeed, AgendaApplyRequest,
    AgendaItemKind, AgendaMutation, AgendaUseCase, BackfillAnchor, BackfillBudget, BackfillCursor,
    BackfillEvidence, BackfillGapUseCase, BackfillLease, BackfillOutcome, BackfillScopeStatus,
    Clock, CommitmentMemory, CommitmentStatus, ConnectionEndReason,
    ConservativeThreadSemanticExtractor, ContentSegment, ContentTrustLevel, ConversationKind,
    ConversationMemoryModeInput, ConversationRef, DeterministicThreadPlanner,
    DeterministicThreadPolicy, DirectoryEvidence, DirectorySnapshot, DirectorySnapshotId,
    DirectorySourceApi, DirectoryStatus, EvaluationCommitResult, EventThreadId,
    FollowUpControlEffectRequest, FollowUpControlStoreError, FollowUpControlTarget,
    FollowUpControlUseCase, FollowUpId, HistoryBackfillSourceT, HistoryCompleteness,
    INITIAL_CANDIDATE_VERSION, InMemoryCheckpointStore, InboundMessageEnvelope,
    IngestMessageOutcome, IngestionGapReason, IngestionGapStatus,
    LegacyNotificationReconciliationConfig, MemoryCandidate, MemoryCandidateBatch,
    MemoryCandidateControlEffectRequest, MemoryCandidateControlStoreError,
    MemoryCandidateControlUseCase, MemoryCandidateExtractorError, MemoryCandidateExtractorT,
    MemoryCandidateId, MemoryCandidateKind, MemoryCandidateSource, MemoryCandidateStatus,
    MemoryCandidateUseCase, MemoryCandidateVersion, MemoryDeleteInput, MemoryFact, MemoryFactId,
    MemoryFactStatus, MemoryPayload, MemoryUseCase, MessageSource, NotificationFailureKind,
    NotificationPolicyEvaluator, NotificationPolicyUseCase, OwnerNotificationContent, PersonMemory,
    PlannerError, PlannerInput, PlannerOutput, PlannerUseCase, ProjectMemory,
    ResponseExpectationControlTarget, ResponseExpectationControlUseCase, ResponseExpectationId,
    ScopeProgress, SecretaryAction, SecretaryActionProposal, SecretaryActionResumeInput,
    SecretaryAgentState, SecretaryApprovalDecision, SourceAccountRef, SourceEventId,
    SourceMessageRef, SystemClock, ThreadActorRef, ThreadLinkCandidateId, ThreadLinkReviewAction,
    ThreadLinkReviewUseCase, ThreadLinkUseCase, ThreadMutationApprovalNode, ThreadMutationDecision,
    ThreadMutationDecisionNode, ThreadMutationEffect, ThreadMutationEffectExecutor,
    ThreadMutationImpact, ThreadMutationKind, ThreadMutationProposalId, ThreadMutationResumeInput,
    ThreadMutationRevertInput, ThreadMutationRevertUseCase, ThreadMutationStoreT,
    ThreadMutationUseCase, ThreadProjectionUseCase, ThreadSemanticUseCase, VerifiedActor,
    VerifiedActorKind, build_mysql_action_store, build_mysql_agenda_store,
    build_mysql_backfill_store, build_mysql_directory_store, build_mysql_follow_up_control_store,
    build_mysql_follow_up_store, build_mysql_inbound_event_store,
    build_mysql_memory_candidate_control_store, build_mysql_memory_candidate_store,
    build_mysql_memory_store, build_mysql_notification_policy_store,
    build_mysql_response_expectation_control_store, build_mysql_retriever_store,
    build_mysql_thread_link_store, build_mysql_thread_mutation_checkpoint_store,
    build_mysql_thread_mutation_store, build_mysql_thread_projection_store,
    build_mysql_thread_semantic_store, candidate_fingerprint,
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

    let retriever = build_mysql_retriever_store(db.clone());
    let pending = retriever
        .list_pending_owner_work(&fact.account, 10)
        .await
        .unwrap();
    assert!(
        pending
            .iter()
            .any(|item| item.source_kind == "follow_up" && item.summary.contains("quote-delivery"))
    );
    let status = retriever.secretary_status(&fact.account).await.unwrap();
    assert_eq!(status.scheduled_follow_up_count, 1);
    assert_eq!(status.pending_evaluation_count, 1);

    // —— source_version 返回与跨账号隔离（version fencing 基础）——
    // FollowUp 必须返回真实 source_version 列值（物化时写入 1），不允许用 0 占位。
    let pending = retriever
        .list_pending_owner_work(&fact.account, 10)
        .await
        .unwrap();
    let follow_up_item = pending
        .iter()
        .find(|item| item.source_kind == "follow_up" && item.summary.contains("quote-delivery"))
        .expect("follow_up 必须出现在待处理事项中");
    assert_eq!(
        follow_up_item.source_version,
        Some(1),
        "follow_up 必须返回真实的 source_version"
    );
    // Agenda 返回真实 version 列值：插入一条 version=3 的议程事项。
    let agenda_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_agenda_items \
         (item_id, account_id, item_kind, title, scheduled_at_unix_secs, timezone_name, \
          item_status, version, created_command_event_id, current_command_event_id, \
          create_idempotency_key) \
         SELECT ?, account_id, 'task', '交付验证议程', 90000, 'UTC', 'scheduled', 3, \
                source_event_id, source_event_id, ? \
         FROM secretary_source_events WHERE source_event_id = ?",
        [
            agenda_id.clone().into(),
            Uuid::new_v4().to_string().into(),
            fact.source_event_ids[0].as_str().into(),
        ],
    ))
    .await
    .unwrap();
    // Outbox 不伪造版本：插入一条 agenda-sourced 失败投递，其来源没有版本列，
    // source_version 必须是 None 而不是 0 或假值。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_outbox \
         (notification_id, account_id, follow_up_id, agenda_item_id, agenda_version, \
          scheduled_at_unix_secs, notification_kind, payload_json, delivery_status) \
         SELECT ?, account_id, NULL, ?, 3, 90000, 'owner_agenda_reminder', '{}', 'failed' \
         FROM secretary_agenda_items WHERE item_id = ?",
        [
            Uuid::new_v4().to_string().into(),
            agenda_id.clone().into(),
            agenda_id.clone().into(),
        ],
    ))
    .await
    .unwrap();
    let pending = retriever
        .list_pending_owner_work(&fact.account, 10)
        .await
        .unwrap();
    let agenda_item = pending
        .iter()
        .find(|item| item.source_kind == "agenda" && item.source_id == agenda_id)
        .expect("agenda 必须出现在待处理事项中");
    assert_eq!(
        agenda_item.source_version,
        Some(3),
        "agenda 必须返回真实的 version 列值"
    );
    let outbox_item = pending
        .iter()
        .find(|item| item.source_kind == "outbox")
        .expect("failed outbox 必须出现在待处理事项中");
    assert_eq!(
        outbox_item.source_version, None,
        "outbox 不得伪造 source_version"
    );
    // 跨账号事项仍不可见：为另一账号插入同状态议程，不得出现在当前账号结果中。
    let other_account_id = format!("follow-up-other-{run_id}");
    let other_envelope = InboundMessageEnvelope::new(
        SourceMessageRef::new(
            MessageSource::NapCat,
            &other_account_id,
            "other-account-source",
        )
        .unwrap(),
        ConversationRef::new(ConversationKind::Group, "delivery-group").unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "bob").unwrap(),
        90_000,
        "另一账号的议程来源事件",
        Vec::new(),
    )
    .unwrap();
    let other_event_id = inbound
        .insert_message_if_absent(&other_envelope)
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let other_agenda_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_agenda_items \
         (item_id, account_id, item_kind, title, scheduled_at_unix_secs, timezone_name, \
          item_status, version, created_command_event_id, current_command_event_id, \
          create_idempotency_key) \
         SELECT ?, id, 'task', '另一账号的议程', 90000, 'UTC', 'scheduled', 7, ?, ?, ? \
         FROM secretary_accounts WHERE source_channel = 'napcat' AND platform_account_id = ?",
        [
            other_agenda_id.clone().into(),
            other_event_id.as_str().into(),
            other_event_id.as_str().into(),
            Uuid::new_v4().to_string().into(),
            other_account_id.into(),
        ],
    ))
    .await
    .unwrap();
    let pending = retriever
        .list_pending_owner_work(&fact.account, 10)
        .await
        .unwrap();
    assert!(
        !pending.iter().any(|item| item.source_id == other_agenda_id),
        "跨账号议程必须对当前账号不可见"
    );

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

    // Owner 只读查询复用同一账号边界，返回参与者、要求、结论、未决问题和来源。
    let account = SourceAccountRef::new(MessageSource::NapCat, &account_id).unwrap();
    let retriever = build_mysql_retriever_store(db.clone());
    let thread_id =
        EventThreadId::new(thread_id_for(&db, &account_id, "semantic-request").await).unwrap();
    let context = retriever
        .thread_context(&account, &thread_id)
        .await
        .unwrap()
        .expect("account-scoped semantic thread must be visible");
    assert_eq!(context.event_count, 5);
    assert_eq!(context.actors.len(), 5);
    assert_eq!(context.claims.len(), 3);
    assert_eq!(context.decisions.len(), 1);
    assert_eq!(context.open_questions.len(), 1);
    assert!(
        context
            .claims
            .iter()
            .all(|claim| !claim.source_event_ids.is_empty())
    );
    let status = retriever.secretary_status(&account).await.unwrap();
    assert!(status.open_thread_count >= 1);
    assert_eq!(status.active_response_expectation_count, 0);
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

// ===== Owner 审批忽略单个 FollowUp（L2 控制闭环） =====

/// 测试用 Planner：固定返回 DismissFollowUp Proposal，不调用 LLM。
struct DismissFollowUpPlanner {
    follow_up_id: FollowUpId,
    expected_source_version: u64,
}

#[async_trait]
impl ActionPlannerT for DismissFollowUpPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::DismissFollowUp {
                    follow_up_id: self.follow_up_id.clone(),
                    expected_source_version: self.expected_source_version,
                    reason: "Owner 确认该跟进不再需要".into(),
                },
                "测试：Owner 审批忽略单个 FollowUp",
                Vec::new(),
                Some("dismiss-follow-up-v1".into()),
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

async fn follow_up_id_for_fact(db: &sea_orm::DatabaseConnection, fact_id: &str) -> String {
    scalar_string(
        db,
        "SELECT item.follow_up_id AS value FROM secretary_follow_up_items item \
         WHERE item.source_memory_fact_id = ?",
        [fact_id],
    )
    .await
    .expect("follow_up must exist for fact")
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn owner_approved_dismiss_follow_up_full_flow_with_version_fencing() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("dismiss-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();

    // 1. 来源化 Commitment FollowUp（source_version=1）
    let commitment_event = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::NapCat, &managed_id, "commitment-1").unwrap(),
                ConversationRef::new(ConversationKind::Group, "dismiss-group").unwrap(),
                VerifiedActor::new(VerifiedActorKind::External, "alice").unwrap(),
                100_000,
                "我会在明天发送报价单",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let memory_store = build_mysql_memory_store(db.clone());
    let memory = MemoryUseCase::new(memory_store.clone());
    let fact = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: managed.clone(),
        subject_key: "commitment:dismiss-quote".into(),
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
            due_at_unix_secs: Some(101_000),
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_500,
        source_event_ids: vec![commitment_event],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory.remember(&fact).await.unwrap();
    let follow_up = personal_secretary::FollowUpUseCase::new(
        build_mysql_follow_up_store(db.clone()),
        memory_store,
    );
    let report = follow_up
        .scan(100_000, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    assert_eq!(report.commitments_materialized, 1);
    let follow_up_id = follow_up_id_for_fact(&db, fact.fact_id.as_str()).await;
    // 2. OwnerCommand 与有效 OwnerBinding
    let command_account_id = format!("dismiss-command-{suffix}");
    let command_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::QqOpenPlatform, &command_account_id, "cmd-1")
                    .unwrap(),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                100_100,
                "忽略这条跟进事项",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_owner_bindings \
         (binding_id, managed_account_id, command_account_id, owner_actor_id, status) \
         SELECT ?, managed.id, command.id, 'owner-openid', 'active' \
         FROM secretary_accounts managed JOIN secretary_accounts command \
         WHERE managed.source_channel = 'napcat' AND managed.platform_account_id = ? \
           AND command.source_channel = 'qq_open_platform' AND command.platform_account_id = ?",
        vec![
            Uuid::new_v4().to_string().into(),
            managed_id.clone().into(),
            command_account_id.clone().into(),
        ],
    ))
    .await
    .unwrap();

    // 现行生产路径：FollowUp 到期后生成 Candidate/Request，由统一策略求值生成
    // policy-owned Outbox；这类行的 follow_up_id 为 NULL，只能经 Candidate 回溯来源。
    let due_report = follow_up
        .scan(101_000, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    assert_eq!(due_report.notification_candidates_created, 1);
    assert_eq!(due_report.notification_evaluation_requests_created, 1);
    let policy = NotificationPolicyUseCase::new(
        build_mysql_notification_policy_store(db.clone()),
        Arc::new(SystemClock),
    );
    assert_eq!(
        policy
            .evaluate_next("dismiss-follow-up-policy", 60, |snapshot| {
                NotificationPolicyEvaluator.evaluate(&snapshot.evaluation_input(101_001).unwrap())
            })
            .await
            .unwrap(),
        Some(EvaluationCommitResult::Applied)
    );
    let outbox_id = scalar_string(
        &db,
        "SELECT outbox.notification_id AS value \
         FROM secretary_notification_outbox outbox \
         JOIN secretary_notification_candidates candidate \
           ON candidate.notification_candidate_id = outbox.notification_candidate_id \
         WHERE candidate.source_kind = 'follow_up' AND candidate.source_id = ?",
        [follow_up_id.as_str()],
    )
    .await
    .expect("policy-owned follow-up outbox must exist");

    // 3. action_run + 初次运行：Planner 生成 DismissFollowUp -> Suspend 等 Owner 审批
    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&command_event_id, "dismiss-v1");
    action_store
        .ensure_action_run(
            &run_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command_event_id.clone(),
                command_text: "忽略这条跟进事项".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: 100_100,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command_event_id.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let control = Arc::new(FollowUpControlUseCase::new(
        build_mysql_follow_up_control_store(db.clone()),
    ));
    let initial = PlannerUseCase::new(
        action_store,
        Arc::new(DismissFollowUpPlanner {
            follow_up_id: FollowUpId::new(follow_up_id.clone()).unwrap(),
            expected_source_version: 1,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(Arc::clone(&control));
    let run = initial
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("run must be claimed");
    assert!(run.suspended, "L2 dismiss follow-up must await approval");
    let checkpoint_id = run
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal_id = run.proposal_id.expect("suspended run must have proposal");

    // 4. 模拟进程重建：全新 PlannerUseCase 与 CheckpointStore，Resume Approve
    let resumed = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(DismissFollowUpPlanner {
            follow_up_id: FollowUpId::new(follow_up_id.clone()).unwrap(),
            expected_source_version: 1,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(control);
    let resumed_report = resumed
        .resume_run(
            &run_id,
            &checkpoint_id,
            SecretaryActionResumeInput {
                proposal_id: proposal_id.clone(),
                decision: SecretaryApprovalDecision::Approve,
                command_source_event_id: command_event_id.clone(),
                approval_source_event_id: None,
            },
        )
        .await
        .expect("approved resume must execute dismiss effect");
    assert!(
        resumed_report.completed,
        "approved resume must complete the run"
    );

    // 5. 断言：dismissed + 版本精确 +1、Outbox suppressed、审计/回执/响应各一条
    let follow_up_status = scalar_string(
        &db,
        "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
        [follow_up_id.as_str()],
    )
    .await;
    assert_eq!(follow_up_status.as_deref(), Some("dismissed"));
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_follow_up_items \
             WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        2,
        "dismiss must bump source_version by exactly 1"
    );
    let outbox_status = scalar_string(
        &db,
        "SELECT delivery_status AS value FROM secretary_notification_outbox \
         WHERE notification_id = ?",
        [outbox_id.as_str()],
    )
    .await;
    assert_eq!(outbox_status.as_deref(), Some("suppressed"));
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT (lease_token IS NULL) AS value FROM secretary_notification_outbox \
             WHERE notification_id = ?",
            [outbox_id.as_str()],
        )
        .await,
        1,
        "suppressed outbox must clear its lease"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        1,
        "dismiss must write exactly one immutable control audit"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "dismiss must persist exactly one effect receipt"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "completed resume must persist one owner response"
    );

    // 6. 第二次 Resume 必须被 Checkpoint CAS 拒绝，且版本/审计不再变化
    assert!(
        resumed
            .resume_run(
                &run_id,
                &checkpoint_id,
                SecretaryActionResumeInput {
                    proposal_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command_event_id.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "checkpoint CAS must reject the second approved resume"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_follow_up_items \
             WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        2,
        "second resume must not move the version again"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        1,
        "second resume must not write another audit"
    );

    // 7. 错误 expected_source_version：不产生业务修改、审计或回执
    let second_event = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::NapCat, &managed_id, "commitment-2").unwrap(),
                ConversationRef::new(ConversationKind::Group, "dismiss-group").unwrap(),
                VerifiedActor::new(VerifiedActorKind::External, "alice").unwrap(),
                103_000,
                "我会在周四前提交设计稿",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let second_fact = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: managed.clone(),
        subject_key: "commitment:dismiss-design".into(),
        payload: MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: managed.clone(),
                actor_id: "alice".into(),
            },
            beneficiary: ThreadActorRef {
                account: managed.clone(),
                actor_id: "owner".into(),
            },
            action: "提交设计稿".into(),
            due_at_unix_secs: Some(104_000),
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_500,
        source_event_ids: vec![second_event],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory.remember(&second_fact).await.unwrap();
    let second_report = follow_up
        .scan(103_000, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    assert_eq!(second_report.commitments_materialized, 1);
    let second_follow_up_id = follow_up_id_for_fact(&db, second_fact.fact_id.as_str()).await;
    assert_ne!(second_follow_up_id, follow_up_id);
    let second_outbox_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_outbox \
         (notification_id, account_id, follow_up_id, scheduled_at_unix_secs, notification_kind, \
          payload_json, delivery_status) \
         SELECT ?, id, ?, 105000, 'owner_reminder', '{}', 'pending' \
         FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
        vec![
            second_outbox_id.clone().into(),
            second_follow_up_id.clone().into(),
            MessageSource::NapCat.as_str().into(),
            managed_id.clone().into(),
        ],
    ))
    .await
    .unwrap();
    let command2_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::QqOpenPlatform, &command_account_id, "cmd-2")
                    .unwrap(),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                103_100,
                "忽略另一条跟进",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let run2_id = ActionRunId::for_owner_command(&command2_event_id, "dismiss-v1");
    build_mysql_action_store(db.clone())
        .ensure_action_run(
            &run2_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command2_event_id.clone(),
                command_text: "忽略另一条跟进".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: 103_100,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command2_event_id.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let claimed2 = build_mysql_action_store(db.clone())
        .claim_pending_run("test-worker", 60, 103_200)
        .await
        .unwrap()
        .expect("second run must be claimable");
    let wrong = Arc::new(FollowUpControlUseCase::new(
        build_mysql_follow_up_control_store(db.clone()),
    ))
    .apply_effect(&{
        let proposal = SecretaryActionProposal::new(
            SecretaryAction::DismissFollowUp {
                follow_up_id: FollowUpId::new(second_follow_up_id.clone()).unwrap(),
                expected_source_version: 99,
                reason: "错误版本".into(),
            },
            "测试错误版本 fencing",
            Vec::new(),
            Some("dismiss-follow-up-wrong-version-v1".into()),
        )
        .unwrap();
        FollowUpControlEffectRequest {
            account: managed,
            command_source_event_id: command2_event_id,
            run_id: run2_id.clone(),
            lease_token: claimed2.lease_token,
            effect_id: format!("effect-wrong-version-{suffix}"),
            proposal_id: proposal.proposal_id.clone(),
            proposal_json: serde_json::to_string(&proposal).unwrap(),
            action: proposal.action,
        }
    })
    .await;
    assert!(
        matches!(wrong, Err(FollowUpControlStoreError::InvalidData(_))),
        "wrong expected_source_version must fail as InvalidData, got {wrong:?}"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_follow_up_items \
             WHERE follow_up_id = ?",
            [second_follow_up_id.as_str()],
        )
        .await,
        1,
        "wrong version must not modify the follow_up"
    );
    let second_outbox_status = scalar_string(
        &db,
        "SELECT delivery_status AS value FROM secretary_notification_outbox \
         WHERE notification_id = ?",
        [second_outbox_id.as_str()],
    )
    .await;
    assert_eq!(
        second_outbox_status.as_deref(),
        Some("pending"),
        "wrong version must not suppress the outbox"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id = ?",
            [second_follow_up_id.as_str()],
        )
        .await,
        0,
        "wrong version must not write a control audit"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run2_id.as_str()],
        )
        .await,
        0,
        "wrong version must not write an effect receipt"
    );
}

// ===== Owner 审批推迟单个 FollowUp（Snooze + 通知重新生成闭环） =====

/// 测试用 Planner：固定返回 SnoozeFollowUp Proposal，不调用 LLM。
struct SnoozeFollowUpPlanner {
    follow_up_id: FollowUpId,
    expected_source_version: u64,
    snooze_until_unix_secs: i64,
}

#[async_trait]
impl ActionPlannerT for SnoozeFollowUpPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::SnoozeFollowUp {
                    follow_up_id: self.follow_up_id.clone(),
                    expected_source_version: self.expected_source_version,
                    snooze_until_unix_secs: self.snooze_until_unix_secs,
                    reason: "Owner 希望晚些再提醒".into(),
                },
                "测试：Owner 审批推迟单个 FollowUp",
                Vec::new(),
                Some("snooze-follow-up-v1".into()),
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

/// 以指定“当前时间”执行一次 FollowUp 扫描，返回扫描报告。
async fn follow_up_scan_at(
    db: &sea_orm::DatabaseConnection,
    now_unix_secs: i64,
) -> personal_secretary::FollowUpScanReport {
    personal_secretary::FollowUpUseCase::new(
        build_mysql_follow_up_store(db.clone()),
        build_mysql_memory_store(db.clone()),
    )
    .scan(now_unix_secs, 86_400, 14_400, 86_400, 100)
    .await
    .unwrap()
}

/// 创建来源化 Commitment 记忆并物化 FollowUp（source_version=1）。
/// 返回（记忆事实, follow_up_id），供后续断言与扫描复用。
async fn commitment_follow_up_fixture(
    db: &sea_orm::DatabaseConnection,
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    managed: &SourceAccountRef,
    managed_id: &str,
    message_id: &str,
    subject_key: &str,
    due_at_unix_secs: i64,
) -> (MemoryFact, String) {
    let commitment_event = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::NapCat, managed_id, message_id).unwrap(),
                ConversationRef::new(ConversationKind::Group, "snooze-group").unwrap(),
                VerifiedActor::new(VerifiedActorKind::External, "alice").unwrap(),
                due_at_unix_secs - 60,
                "我会准时完成这份交付",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let memory_store = build_mysql_memory_store(db.clone());
    let memory = MemoryUseCase::new(memory_store.clone());
    let fact = MemoryFact {
        fact_id: MemoryFactId::generate(),
        account: managed.clone(),
        subject_key: subject_key.into(),
        payload: MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: managed.clone(),
                actor_id: "alice".into(),
            },
            beneficiary: ThreadActorRef {
                account: managed.clone(),
                actor_id: "owner".into(),
            },
            action: "按时交付".into(),
            due_at_unix_secs: Some(due_at_unix_secs),
            status: CommitmentStatus::Pending,
            completion_source_event_id: None,
        }),
        status: MemoryFactStatus::Confirmed,
        confidence_bps: 9_500,
        source_event_ids: vec![commitment_event],
        valid_until_unix_secs: None,
        supersedes_fact_id: None,
    };
    memory.remember(&fact).await.unwrap();
    let follow_up = personal_secretary::FollowUpUseCase::new(
        build_mysql_follow_up_store(db.clone()),
        memory_store,
    );
    let report = follow_up
        .scan(due_at_unix_secs - 1, 86_400, 14_400, 86_400, 100)
        .await
        .unwrap();
    assert_eq!(report.commitments_materialized, 1);
    let follow_up_id = follow_up_id_for_fact(db, fact.fact_id.as_str()).await;
    (fact, follow_up_id)
}

/// 插入 OwnerCommand 并建立有效的 owner binding，返回命令事件 ID。
async fn owner_command_with_binding(
    db: &sea_orm::DatabaseConnection,
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    managed_id: &str,
    command_account_id: &str,
    message_id: &str,
    text: &str,
    occurred_at_unix_secs: i64,
) -> String {
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
        .unwrap()
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
           AND command.source_channel = 'qq_open_platform' AND command.platform_account_id = ?",
            vec![
                Uuid::new_v4().to_string().into(),
                managed_id.to_owned().into(),
                command_account_id.to_owned().into(),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(
        inserted.rows_affected(),
        1,
        "snooze fixture must create exactly one active OwnerBinding"
    );
    command_event_id.as_str().to_owned()
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn owner_approved_snooze_follow_up_full_flow_with_notification_regeneration() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("snooze-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();
    // 时间以真实时钟为基准：due 已过去、snooze 目标在未来，且与数据库 UTC 时间留有裕量。
    let now = SystemClock.now_unix_secs();
    let base_due = now - 3600;
    let snooze_until = now + 7200;

    // 1. 来源化 FollowUp，到期后真实生成 Candidate/Request，统一策略求值生成
    //    policy-owned Outbox（v1）；另插入一条 legacy 直接关联的 pending Outbox。
    let (_fact, follow_up_id) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "snooze-commitment",
        "commitment:snooze-delivery",
        base_due,
    )
    .await;
    // Owner 收件绑定必须在策略求值前存在；Remind 的原子提交会在最终事务中
    // 复验接收方，不能依赖后续审批步骤才补建绑定。此时 managed account
    // 已由来源化 FollowUp fixture 创建。
    let command_account_id = format!("snooze-command-{suffix}");
    let command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "snooze-cmd-1",
        "推迟这条跟进事项",
        base_due + 60,
    )
    .await;
    let command_event = personal_secretary::SourceEventId::new(command_event_id).unwrap();
    let due_report = follow_up_scan_at(&db, base_due).await;
    assert_eq!(due_report.notification_candidates_created, 1);
    assert_eq!(due_report.notification_evaluation_requests_created, 1);
    let policy = NotificationPolicyUseCase::new(
        build_mysql_notification_policy_store(db.clone()),
        Arc::new(SystemClock),
    );
    assert_eq!(
        policy
            .evaluate_next("snooze-follow-up-policy", 60, |snapshot| {
                NotificationPolicyEvaluator
                    .evaluate(&snapshot.evaluation_input(base_due + 1).unwrap())
            })
            .await
            .unwrap(),
        Some(EvaluationCommitResult::Applied)
    );
    let outbox_v1_id = scalar_string(
        &db,
        "SELECT outbox.notification_id AS value \
         FROM secretary_notification_outbox outbox \
         JOIN secretary_notification_candidates candidate \
           ON candidate.notification_candidate_id = outbox.notification_candidate_id \
         WHERE candidate.source_kind = 'follow_up' AND candidate.source_id = ?",
        [follow_up_id.as_str()],
    )
    .await
    .expect("policy-owned follow-up outbox must exist");
    let legacy_outbox_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_outbox \
         (notification_id, account_id, follow_up_id, scheduled_at_unix_secs, notification_kind, \
          payload_json, delivery_status) \
         SELECT ?, id, ?, ?, 'owner_reminder', '{}', 'pending' \
         FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
        vec![
            legacy_outbox_id.clone().into(),
            follow_up_id.clone().into(),
            base_due.into(),
            MessageSource::NapCat.as_str().into(),
            managed_id.clone().into(),
        ],
    ))
    .await
    .unwrap();

    // 2. 基于前述 OwnerCommand/OwnerBinding 创建 action_run，初次运行 Suspend。
    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&command_event, "snooze-v1");
    action_store
        .ensure_action_run(
            &run_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command_event.clone(),
                command_text: "推迟这条跟进事项".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: base_due + 60,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command_event.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let control = Arc::new(FollowUpControlUseCase::new(
        build_mysql_follow_up_control_store(db.clone()),
    ));
    let initial = PlannerUseCase::new(
        action_store,
        Arc::new(SnoozeFollowUpPlanner {
            follow_up_id: FollowUpId::new(follow_up_id.clone()).unwrap(),
            expected_source_version: 1,
            snooze_until_unix_secs: snooze_until,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(Arc::clone(&control));
    let run = initial
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("run must be claimed");
    assert!(run.suspended, "L2 snooze follow-up must await approval");
    let checkpoint_id = run
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal_id = run.proposal_id.expect("suspended run must have proposal");

    // 3. 模拟进程重建后 Resume Approve
    let resumed = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(SnoozeFollowUpPlanner {
            follow_up_id: FollowUpId::new(follow_up_id.clone()).unwrap(),
            expected_source_version: 1,
            snooze_until_unix_secs: snooze_until,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(control);
    let resumed_report = resumed
        .resume_run(
            &run_id,
            &checkpoint_id,
            SecretaryActionResumeInput {
                proposal_id: proposal_id.clone(),
                decision: SecretaryApprovalDecision::Approve,
                command_source_event_id: command_event.clone(),
                approval_source_event_id: None,
            },
        )
        .await
        .expect("approved resume must execute snooze effect");
    assert!(
        resumed_report.completed,
        "approved resume must complete the run"
    );

    // 4. 断言：仍 scheduled、due 精确变为 snooze_until、版本精确 +1、
    //    旧 policy-owned 与 legacy Outbox 均 suppressed、一条 snooze 审计、
    //    一条回执、一条响应。
    assert_eq!(
        scalar_string(
            &db,
            "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await
        .as_deref(),
        Some("scheduled"),
        "snooze must keep the follow_up scheduled"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT due_at_unix_secs AS value FROM secretary_follow_up_items \
             WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        snooze_until,
        "snooze must set due exactly to snooze_until"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_follow_up_items \
             WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        2,
        "snooze must bump source_version by exactly 1"
    );
    for (outbox_id, label) in [
        (&outbox_v1_id, "policy-owned"),
        (&legacy_outbox_id, "legacy"),
    ] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT delivery_status AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_id.as_str()],
            )
            .await
            .as_deref(),
            Some("suppressed"),
            "{label} outbox must be suppressed after snooze"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT (lease_token IS NULL) AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_id.as_str()],
            )
            .await,
            1,
            "suppressed {label} outbox must clear its lease"
        );
    }
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        1,
        "snooze must write exactly one immutable control audit"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT control_kind AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await
        .as_deref(),
        Some("snooze")
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(previous_source_version AS SIGNED) AS value \
             FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        1
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(current_source_version AS SIGNED) AS value \
             FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        2
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(previous_due_at_unix_secs AS SIGNED) AS value \
             FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        base_due
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(current_due_at_unix_secs AS SIGNED) AS value \
             FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        snooze_until
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "snooze must persist exactly one effect receipt"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "completed resume must persist one owner response"
    );

    // 5. 第二次 Resume 必须被 Checkpoint CAS 拒绝，版本/审计不再变化
    assert!(
        resumed
            .resume_run(
                &run_id,
                &checkpoint_id,
                SecretaryActionResumeInput {
                    proposal_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command_event,
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "checkpoint CAS must reject the second approved resume"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_follow_up_items \
             WHERE follow_up_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        2,
        "second resume must not move the version again"
    );

    // 6. 新 due 前扫描：不得产生新版本 Candidate/Outbox
    let before_report = follow_up_scan_at(&db, snooze_until - 60).await;
    assert_eq!(before_report.notification_candidates_created, 0);
    assert_eq!(before_report.notification_evaluation_requests_created, 0);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_notification_candidates \
             WHERE source_kind = 'follow_up' AND source_id = ?",
            [follow_up_id.as_str()],
        )
        .await,
        1,
        "scan before the new due must not create a new-version candidate"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value \
             FROM secretary_notification_outbox outbox \
             LEFT JOIN secretary_notification_candidates candidate \
               ON candidate.notification_candidate_id = outbox.notification_candidate_id \
             WHERE outbox.follow_up_id = ? OR \
                   (candidate.source_kind = 'follow_up' AND candidate.source_id = ?)",
            [follow_up_id.as_str(), follow_up_id.as_str()],
        )
        .await,
        2,
        "scan before the new due must not create a new outbox occurrence"
    );

    // 7. 到达新 due 后扫描并统一求值：生成新版本 Candidate、新 Decision 与
    //    新 policy-owned Outbox；旧 Outbox 保持 suppressed，不被复活。
    let after_report = follow_up_scan_at(&db, snooze_until + 60).await;
    assert_eq!(after_report.notification_candidates_created, 1);
    assert_eq!(after_report.notification_evaluation_requests_created, 1);
    assert_eq!(
        policy
            .evaluate_next("snooze-follow-up-policy", 60, |snapshot| {
                NotificationPolicyEvaluator
                    .evaluate(&snapshot.evaluation_input(snooze_until + 61).unwrap())
            })
            .await
            .unwrap(),
        Some(EvaluationCommitResult::Applied)
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_notification_candidates \
             WHERE source_kind = 'follow_up' AND source_id = ? AND source_version = 2",
            [follow_up_id.as_str()],
        )
        .await,
        1,
        "new due must create a source_version=2 candidate"
    );
    let outbox_v2_id = scalar_string(
        &db,
        "SELECT outbox.notification_id AS value \
         FROM secretary_notification_outbox outbox \
         JOIN secretary_notification_candidates candidate \
           ON candidate.notification_candidate_id = outbox.notification_candidate_id \
         WHERE candidate.source_kind = 'follow_up' AND candidate.source_id = ? \
           AND candidate.source_version = 2",
        [follow_up_id.as_str()],
    )
    .await
    .expect("new policy-owned outbox must exist");
    assert_ne!(outbox_v2_id, outbox_v1_id);
    assert_eq!(
        scalar_string(
            &db,
            "SELECT delivery_status AS value FROM secretary_notification_outbox \
             WHERE notification_id = ?",
            [outbox_v2_id.as_str()],
        )
        .await
        .as_deref(),
        Some("pending"),
        "new outbox occurrence must be pending"
    );
    for (outbox_id, label) in [
        (&outbox_v1_id, "policy-owned v1"),
        (&legacy_outbox_id, "legacy"),
    ] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT delivery_status AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_id.as_str()],
            )
            .await
            .as_deref(),
            Some("suppressed"),
            "{label} outbox must stay suppressed, never resurrected"
        );
    }
}

// ===== Owner 审批批量忽略 FollowUp（all-or-nothing + 幂等） =====

/// 测试用 Planner：固定返回 DismissFollowUps Proposal，不调用 LLM。
struct DismissFollowUpsPlanner {
    targets: Vec<FollowUpControlTarget>,
}

#[async_trait]
impl ActionPlannerT for DismissFollowUpsPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::DismissFollowUps {
                    targets: self.targets.clone(),
                    reason: "Owner 确认这些跟进事项不再需要".into(),
                },
                "测试：Owner 审批批量忽略 FollowUp",
                Vec::new(),
                Some("batch-dismiss-follow-ups-v1".into()),
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

/// 插入一条与 FollowUp 直接关联的 legacy pending Outbox，并断言恰好写入一行。
async fn legacy_pending_outbox_for(
    db: &sea_orm::DatabaseConnection,
    managed_id: &str,
    follow_up_id: &str,
    scheduled_at_unix_secs: i64,
    outbox_id: &str,
) {
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_notification_outbox \
             (notification_id, account_id, follow_up_id, scheduled_at_unix_secs, notification_kind, \
              payload_json, delivery_status) \
             SELECT ?, id, ?, ?, 'owner_reminder', '{}', 'pending' \
             FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
            vec![
                outbox_id.to_owned().into(),
                follow_up_id.to_owned().into(),
                scheduled_at_unix_secs.into(),
                MessageSource::NapCat.as_str().into(),
                managed_id.to_owned().into(),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(
        inserted.rows_affected(),
        1,
        "legacy outbox fixture must insert exactly one row"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn owner_approved_batch_dismiss_follow_ups_is_atomic_and_idempotent() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("batch-dismiss-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();
    let now = SystemClock.now_unix_secs();
    let base_due = now - 3600;

    // ===== 成功路径：两条 FollowUp，一条 policy-owned、一条 legacy Outbox =====
    // 1a. 来源化 FollowUp A（已到期），先建 OwnerCommand/Binding 再生成 policy outbox，
    //     保证策略 Remind 的接收方复验有绑定可查。
    let (_fact_a, follow_up_a) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "batch-commitment-a",
        "commitment:batch-a",
        base_due,
    )
    .await;
    let command_account_id = format!("batch-dismiss-command-{suffix}");
    let command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "batch-cmd-1",
        "把这几条跟进都忽略",
        base_due + 60,
    )
    .await;
    let command_event = personal_secretary::SourceEventId::new(command_event_id).unwrap();
    // 1b. A 到期后真实生成 Candidate/Request，统一策略求值生成 policy-owned Outbox
    //     （follow_up_id 为 NULL，只能经 Candidate 回溯）。
    let due_report = follow_up_scan_at(&db, base_due).await;
    assert_eq!(due_report.notification_candidates_created, 1);
    assert_eq!(due_report.notification_evaluation_requests_created, 1);
    let policy = NotificationPolicyUseCase::new(
        build_mysql_notification_policy_store(db.clone()),
        Arc::new(SystemClock),
    );
    assert_eq!(
        policy
            .evaluate_next("batch-dismiss-policy", 60, |snapshot| {
                NotificationPolicyEvaluator
                    .evaluate(&snapshot.evaluation_input(base_due + 1).unwrap())
            })
            .await
            .unwrap(),
        Some(EvaluationCommitResult::Applied)
    );
    let outbox_a_id = scalar_string(
        &db,
        "SELECT outbox.notification_id AS value \
         FROM secretary_notification_outbox outbox \
         JOIN secretary_notification_candidates candidate \
           ON candidate.notification_candidate_id = outbox.notification_candidate_id \
         WHERE candidate.source_kind = 'follow_up' AND candidate.source_id = ?",
        [follow_up_a.as_str()],
    )
    .await
    .expect("policy-owned follow-up outbox must exist");
    // 1c. 来源化 FollowUp B（未到期），插一条 legacy pending Outbox。
    let (_fact_b, follow_up_b) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "batch-commitment-b",
        "commitment:batch-b",
        base_due + 7200,
    )
    .await;
    let legacy_outbox_b_id = Uuid::new_v4().to_string();
    legacy_pending_outbox_for(
        &db,
        &managed_id,
        &follow_up_b,
        base_due + 7200,
        &legacy_outbox_b_id,
    )
    .await;

    // 2. action_run + 初次运行：Planner 生成 DismissFollowUps -> Suspend 等 Owner 审批。
    let targets = vec![
        FollowUpControlTarget {
            follow_up_id: FollowUpId::new(follow_up_a.clone()).unwrap(),
            expected_source_version: 1,
        },
        FollowUpControlTarget {
            follow_up_id: FollowUpId::new(follow_up_b.clone()).unwrap(),
            expected_source_version: 1,
        },
    ];
    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&command_event, "batch-dismiss-v1");
    action_store
        .ensure_action_run(
            &run_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command_event.clone(),
                command_text: "把这几条跟进都忽略".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: base_due + 60,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command_event.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let control = Arc::new(FollowUpControlUseCase::new(
        build_mysql_follow_up_control_store(db.clone()),
    ));
    let initial = PlannerUseCase::new(
        action_store,
        Arc::new(DismissFollowUpsPlanner {
            targets: targets.clone(),
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(Arc::clone(&control));
    let run = initial
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("run must be claimed");
    assert!(run.suspended, "L2 batch dismiss must await approval");
    let checkpoint_id = run
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal_id = run.proposal_id.expect("suspended run must have proposal");

    // 3. 模拟进程重建后 Resume Approve。
    let resumed = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(DismissFollowUpsPlanner { targets }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(control);
    let resumed_report = resumed
        .resume_run(
            &run_id,
            &checkpoint_id,
            SecretaryActionResumeInput {
                proposal_id: proposal_id.clone(),
                decision: SecretaryApprovalDecision::Approve,
                command_source_event_id: command_event.clone(),
                approval_source_event_id: None,
            },
        )
        .await
        .expect("approved resume must execute batch dismiss effect");
    assert!(
        resumed_report.completed,
        "approved resume must complete the run"
    );

    // 4. 断言成功语义：全部 dismissed、版本精确 +1、两类 Outbox suppressed 且租约清空、
    //    每条目标一条审计且共享同一 effect_id、一条回执、一条响应。
    for (id, label) in [(&follow_up_a, "A"), (&follow_up_b, "B")] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await
            .as_deref(),
            Some("dismissed"),
            "follow-up {label} must be dismissed"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_follow_up_items \
                 WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            2,
            "follow-up {label} version must bump by exactly 1"
        );
    }
    for (outbox_id, label) in [
        (&outbox_a_id, "policy-owned"),
        (&legacy_outbox_b_id, "legacy"),
    ] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT delivery_status AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_id.as_str()],
            )
            .await
            .as_deref(),
            Some("suppressed"),
            "{label} outbox must be suppressed"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT (lease_token IS NULL) AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_id.as_str()],
            )
            .await,
            1,
            "suppressed {label} outbox must clear its lease"
        );
    }
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(DISTINCT effect_id) AS SIGNED) AS value \
             FROM secretary_follow_up_owner_controls WHERE follow_up_id IN (?, ?)",
            [follow_up_a.as_str(), follow_up_b.as_str()],
        )
        .await,
        1,
        "batch audit rows must share one effect_id"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id IN (?, ?)",
            [follow_up_a.as_str(), follow_up_b.as_str()],
        )
        .await,
        2,
        "one audit row per target"
    );
    for (id, label) in [(&follow_up_a, "A"), (&follow_up_b, "B")] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT control_kind AS value FROM secretary_follow_up_owner_controls \
                 WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await
            .as_deref(),
            Some("dismiss"),
            "audit for {label} must be dismiss kind"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(previous_source_version AS SIGNED) AS value \
                 FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            1,
            "audit for {label} must record previous version 1"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(current_source_version AS SIGNED) AS value \
                 FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            2,
            "audit for {label} must record current version 2"
        );
    }
    let receipt_effect_id = scalar_string(
        &db,
        "SELECT effect_id AS value FROM secretary_action_effect_receipts WHERE run_id = ?",
        [run_id.as_str()],
    )
    .await
    .expect("batch must persist one effect receipt");
    let audit_effect_id = scalar_exactly_one_string(
        &db,
        "SELECT effect_id AS value FROM secretary_follow_up_owner_controls \
         WHERE follow_up_id = ? AND control_kind = 'dismiss'",
        [follow_up_a.as_str()],
        "audit row for A must be unique",
    )
    .await;
    assert_eq!(
        receipt_effect_id, audit_effect_id,
        "audit effect_id must match the receipt effect_id"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "batch must persist exactly one effect receipt"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "completed resume must persist one owner response"
    );

    // 5. 第二次 Resume 被 Checkpoint CAS 拒绝，计数不再增加。
    assert!(
        resumed
            .resume_run(
                &run_id,
                &checkpoint_id,
                SecretaryActionResumeInput {
                    proposal_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command_event.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "checkpoint CAS must reject the second approved resume"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id IN (?, ?)",
            [follow_up_a.as_str(), follow_up_b.as_str()],
        )
        .await,
        2,
        "second resume must not write another audit"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "second resume must not write another receipt"
    );

    // ===== 原子失败路径：第二个目标版本错误，整个批次必须全回滚 =====
    // 6a. 两个新的来源化 FollowUp（未到期），C 带一条 legacy pending Outbox。
    let (_fact_c, follow_up_c) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "batch-commitment-c",
        "commitment:batch-c",
        base_due + 7200,
    )
    .await;
    let (_fact_d, follow_up_d) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "batch-commitment-d",
        "commitment:batch-d",
        base_due + 7200,
    )
    .await;
    let legacy_outbox_c_id = Uuid::new_v4().to_string();
    legacy_pending_outbox_for(
        &db,
        &managed_id,
        &follow_up_c,
        base_due + 7200,
        &legacy_outbox_c_id,
    )
    .await;
    // 6b. 新 OwnerCommand（复用既有 binding，不再插入新 binding，保持恰好一个 active）。
    let command2_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(
                    MessageSource::QqOpenPlatform,
                    &command_account_id,
                    "batch-cmd-2",
                )
                .unwrap(),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                base_due + 120,
                "忽略另外两条跟进",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let run2_id = ActionRunId::for_owner_command(&command2_event_id, "batch-dismiss-v1");
    build_mysql_action_store(db.clone())
        .ensure_action_run(
            &run2_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command2_event_id.clone(),
                command_text: "忽略另外两条跟进".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: base_due + 120,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command2_event_id.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let failing = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(DismissFollowUpsPlanner {
            targets: vec![
                FollowUpControlTarget {
                    follow_up_id: FollowUpId::new(follow_up_c.clone()).unwrap(),
                    expected_source_version: 1,
                },
                FollowUpControlTarget {
                    follow_up_id: FollowUpId::new(follow_up_d.clone()).unwrap(),
                    expected_source_version: 99,
                },
            ],
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(Arc::new(FollowUpControlUseCase::new(
        build_mysql_follow_up_control_store(db.clone()),
    )));
    let run2 = failing
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("second run must be claimed");
    assert!(run2.suspended, "L2 batch dismiss must await approval first");
    let checkpoint2_id = run2
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal2_id = run2.proposal_id.expect("suspended run must have proposal");
    assert!(
        failing
            .resume_run(
                &run2_id,
                &checkpoint2_id,
                SecretaryActionResumeInput {
                    proposal_id: proposal2_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command2_event_id.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "wrong expected_source_version must fail the whole batch"
    );

    // 6c. 全回滚断言：两条 FollowUp 都未变化、Outbox 未压制、无审计、无回执、无响应。
    for (id, label) in [(&follow_up_c, "C"), (&follow_up_d, "D")] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await
            .as_deref(),
            Some("scheduled"),
            "follow-up {label} must stay scheduled after atomic failure"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_follow_up_items \
                 WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            1,
            "follow-up {label} version must stay unchanged after atomic failure"
        );
    }
    assert_eq!(
        scalar_string(
            &db,
            "SELECT delivery_status AS value FROM secretary_notification_outbox \
             WHERE notification_id = ?",
            [legacy_outbox_c_id.as_str()],
        )
        .await
        .as_deref(),
        Some("pending"),
        "outbox must not be suppressed after atomic failure"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id IN (?, ?)",
            [follow_up_c.as_str(), follow_up_d.as_str()],
        )
        .await,
        0,
        "no batch control audit after atomic failure"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run2_id.as_str()],
        )
        .await,
        0,
        "no effect receipt after atomic failure"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run2_id.as_str()],
        )
        .await,
        0,
        "no optimistic success response after atomic failure"
    );
}

// ===== Owner 审批批量推迟 FollowUp（all-or-nothing + 通知再生） =====

/// 测试用 Planner：固定返回 SnoozeFollowUps Proposal，不调用 LLM。
struct SnoozeFollowUpsPlanner {
    targets: Vec<FollowUpControlTarget>,
    snooze_until_unix_secs: i64,
}

#[async_trait]
impl ActionPlannerT for SnoozeFollowUpsPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::SnoozeFollowUps {
                    targets: self.targets.clone(),
                    snooze_until_unix_secs: self.snooze_until_unix_secs,
                    reason: "Owner 希望统一晚些再提醒".into(),
                },
                "测试：Owner 审批批量推迟 FollowUp",
                Vec::new(),
                Some("batch-snooze-follow-ups-v1".into()),
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn owner_approved_batch_snooze_follow_ups_is_atomic_and_regenerates_notifications() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("batch-snooze-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();
    // 时间以真实时钟为基准：A 已到期、B 未到期且早于共同新时间，snooze 目标在未来。
    let now = SystemClock.now_unix_secs();
    let base_due = now - 3600;
    let snooze_until = now + 7200;

    // ===== 成功路径：两条 FollowUp（不同 due），一条 policy-owned、一条 legacy Outbox =====
    // 1a. 来源化 FollowUp A（已到期），先建 OwnerCommand/Binding 再生成 policy outbox，
    //     保证策略 Remind 的接收方复验有绑定可查。
    let (_fact_a, follow_up_a) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "batch-snooze-commitment-a",
        "commitment:batch-snooze-a",
        base_due,
    )
    .await;
    let command_account_id = format!("batch-snooze-command-{suffix}");
    let command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "batch-snooze-cmd-1",
        "把这几条跟进都推迟",
        base_due + 60,
    )
    .await;
    let command_event = personal_secretary::SourceEventId::new(command_event_id).unwrap();
    // 1b. A 到期后真实生成 Candidate/Request，统一策略求值生成 policy-owned Outbox
    //     （follow_up_id 为 NULL，只能经 Candidate 回溯）。
    let due_report = follow_up_scan_at(&db, base_due).await;
    assert_eq!(due_report.notification_candidates_created, 1);
    assert_eq!(due_report.notification_evaluation_requests_created, 1);
    let policy = NotificationPolicyUseCase::new(
        build_mysql_notification_policy_store(db.clone()),
        Arc::new(SystemClock),
    );
    assert_eq!(
        policy
            .evaluate_next("batch-snooze-policy", 60, |snapshot| {
                NotificationPolicyEvaluator
                    .evaluate(&snapshot.evaluation_input(base_due + 1).unwrap())
            })
            .await
            .unwrap(),
        Some(EvaluationCommitResult::Applied)
    );
    let outbox_a_v1_id = scalar_string(
        &db,
        "SELECT outbox.notification_id AS value \
         FROM secretary_notification_outbox outbox \
         JOIN secretary_notification_candidates candidate \
           ON candidate.notification_candidate_id = outbox.notification_candidate_id \
         WHERE candidate.source_kind = 'follow_up' AND candidate.source_id = ?",
        [follow_up_a.as_str()],
    )
    .await
    .expect("policy-owned follow-up outbox must exist");
    // 1c. 来源化 FollowUp B（未到期），插一条 legacy pending Outbox。
    let (_fact_b, follow_up_b) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "batch-snooze-commitment-b",
        "commitment:batch-snooze-b",
        base_due + 7200,
    )
    .await;
    let legacy_outbox_b_id = Uuid::new_v4().to_string();
    legacy_pending_outbox_for(
        &db,
        &managed_id,
        &follow_up_b,
        base_due + 7200,
        &legacy_outbox_b_id,
    )
    .await;

    // 2. action_run + 初次运行：Planner 生成 SnoozeFollowUps -> Suspend 等 Owner 审批。
    let targets = vec![
        FollowUpControlTarget {
            follow_up_id: FollowUpId::new(follow_up_a.clone()).unwrap(),
            expected_source_version: 1,
        },
        FollowUpControlTarget {
            follow_up_id: FollowUpId::new(follow_up_b.clone()).unwrap(),
            expected_source_version: 1,
        },
    ];
    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&command_event, "batch-snooze-v1");
    action_store
        .ensure_action_run(
            &run_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command_event.clone(),
                command_text: "把这几条跟进都推迟".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: base_due + 60,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command_event.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let control = Arc::new(FollowUpControlUseCase::new(
        build_mysql_follow_up_control_store(db.clone()),
    ));
    let initial = PlannerUseCase::new(
        action_store,
        Arc::new(SnoozeFollowUpsPlanner {
            targets: targets.clone(),
            snooze_until_unix_secs: snooze_until,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(Arc::clone(&control));
    let run = initial
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("run must be claimed");
    assert!(run.suspended, "L2 batch snooze must await approval");
    let checkpoint_id = run
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal_id = run.proposal_id.expect("suspended run must have proposal");

    // 3. 模拟进程重建后 Resume Approve。
    let resumed = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(SnoozeFollowUpsPlanner {
            targets,
            snooze_until_unix_secs: snooze_until,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(control);
    let resumed_report = resumed
        .resume_run(
            &run_id,
            &checkpoint_id,
            SecretaryActionResumeInput {
                proposal_id: proposal_id.clone(),
                decision: SecretaryApprovalDecision::Approve,
                command_source_event_id: command_event.clone(),
                approval_source_event_id: None,
            },
        )
        .await
        .expect("approved resume must execute batch snooze effect");
    assert!(
        resumed_report.completed,
        "approved resume must complete the run"
    );

    // 4. 断言成功语义：仍 scheduled、due 精确变为共同 snooze_until、版本精确 +1、
    //    两类 Outbox suppressed 且租约清空、每目标一条 snooze 审计（前后 due/版本
    //    正确）且共享同一 effect_id、一条回执、一条响应。
    for (id, label) in [(&follow_up_a, "A"), (&follow_up_b, "B")] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await
            .as_deref(),
            Some("scheduled"),
            "follow-up {label} must stay scheduled after batch snooze"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT due_at_unix_secs AS value FROM secretary_follow_up_items \
                 WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            snooze_until,
            "follow-up {label} due must be set exactly to the common snooze time"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_follow_up_items \
                 WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            2,
            "follow-up {label} version must bump by exactly 1"
        );
    }
    for (outbox_id, label) in [
        (&outbox_a_v1_id, "policy-owned"),
        (&legacy_outbox_b_id, "legacy"),
    ] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT delivery_status AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_id.as_str()],
            )
            .await
            .as_deref(),
            Some("suppressed"),
            "{label} outbox must be suppressed after batch snooze"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT (lease_token IS NULL) AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_id.as_str()],
            )
            .await,
            1,
            "suppressed {label} outbox must clear its lease"
        );
    }
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(DISTINCT effect_id) AS SIGNED) AS value \
             FROM secretary_follow_up_owner_controls WHERE follow_up_id IN (?, ?)",
            [follow_up_a.as_str(), follow_up_b.as_str()],
        )
        .await,
        1,
        "batch audit rows must share one effect_id"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id IN (?, ?)",
            [follow_up_a.as_str(), follow_up_b.as_str()],
        )
        .await,
        2,
        "one audit row per target"
    );
    for (id, label, old_due) in [
        (&follow_up_a, "A", base_due),
        (&follow_up_b, "B", base_due + 7200),
    ] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT control_kind AS value FROM secretary_follow_up_owner_controls \
                 WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await
            .as_deref(),
            Some("snooze"),
            "audit for {label} must be snooze kind"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(previous_source_version AS SIGNED) AS value \
                 FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            1,
            "audit for {label} must record previous version 1"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(current_source_version AS SIGNED) AS value \
                 FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            2,
            "audit for {label} must record current version 2"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(previous_due_at_unix_secs AS SIGNED) AS value \
                 FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            old_due,
            "audit for {label} must record the pre-snooze due"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(current_due_at_unix_secs AS SIGNED) AS value \
                 FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            snooze_until,
            "audit for {label} must record the new common due"
        );
    }
    let receipt_effect_id = scalar_string(
        &db,
        "SELECT effect_id AS value FROM secretary_action_effect_receipts WHERE run_id = ?",
        [run_id.as_str()],
    )
    .await
    .expect("batch must persist one effect receipt");
    let audit_effect_id = scalar_exactly_one_string(
        &db,
        "SELECT effect_id AS value FROM secretary_follow_up_owner_controls \
         WHERE follow_up_id = ? AND control_kind = 'snooze'",
        [follow_up_a.as_str()],
        "audit row for A must be unique",
    )
    .await;
    assert_eq!(
        receipt_effect_id, audit_effect_id,
        "audit effect_id must match the receipt effect_id"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "batch must persist exactly one effect receipt"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "completed resume must persist one owner response"
    );

    // 5. 第二次 Resume 被 Checkpoint CAS 拒绝，审计不再变化。
    assert!(
        resumed
            .resume_run(
                &run_id,
                &checkpoint_id,
                SecretaryActionResumeInput {
                    proposal_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command_event.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "checkpoint CAS must reject the second approved resume"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id IN (?, ?)",
            [follow_up_a.as_str(), follow_up_b.as_str()],
        )
        .await,
        2,
        "second resume must not write another audit"
    );

    // 6. 新 due 前扫描：不得产生新版本 Candidate/Outbox；旧 Outbox 保持 suppressed。
    let before_report = follow_up_scan_at(&db, snooze_until - 60).await;
    assert_eq!(before_report.notification_candidates_created, 0);
    assert_eq!(before_report.notification_evaluation_requests_created, 0);
    for (id, label) in [(&follow_up_a, "A"), (&follow_up_b, "B")] {
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_notification_candidates \
                 WHERE source_kind = 'follow_up' AND source_id = ? AND source_version = 1",
                [id.as_str()],
            )
            .await,
            if label == "A" { 1 } else { 0 },
            "scan before the new due must not create a new-version candidate for {label}"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(COUNT(*) AS SIGNED) AS value \
                 FROM secretary_notification_outbox outbox \
                 LEFT JOIN secretary_notification_candidates candidate \
                   ON candidate.notification_candidate_id = outbox.notification_candidate_id \
                 WHERE outbox.follow_up_id = ? OR \
                       (candidate.source_kind = 'follow_up' AND candidate.source_id = ?)",
                [id.as_str(), id.as_str()],
            )
            .await,
            1,
            "scan before the new due must not create a new outbox occurrence for {label}"
        );
    }

    // 7. 到达新 due 后扫描并统一求值：每个目标生成新版本 Candidate、新 Decision 与
    //    新 policy-owned Outbox；旧 Outbox 保持 suppressed，不被复活。
    let after_report = follow_up_scan_at(&db, snooze_until + 60).await;
    assert_eq!(after_report.notification_candidates_created, 2);
    assert_eq!(after_report.notification_evaluation_requests_created, 2);
    for _ in 0..2 {
        assert_eq!(
            policy
                .evaluate_next("batch-snooze-policy", 60, |snapshot| {
                    NotificationPolicyEvaluator
                        .evaluate(&snapshot.evaluation_input(snooze_until + 61).unwrap())
                })
                .await
                .unwrap(),
            Some(EvaluationCommitResult::Applied)
        );
    }
    for (id, label) in [(&follow_up_a, "A"), (&follow_up_b, "B")] {
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_notification_candidates \
                 WHERE source_kind = 'follow_up' AND source_id = ? AND source_version = 2",
                [id.as_str()],
            )
            .await,
            1,
            "new due must create a source_version=2 candidate for {label}"
        );
        let outbox_v2_id = scalar_string(
            &db,
            "SELECT outbox.notification_id AS value \
             FROM secretary_notification_outbox outbox \
             JOIN secretary_notification_candidates candidate \
               ON candidate.notification_candidate_id = outbox.notification_candidate_id \
             WHERE candidate.source_kind = 'follow_up' AND candidate.source_id = ? \
               AND candidate.source_version = 2",
            [id.as_str()],
        )
        .await
        .expect("new policy-owned outbox must exist");
        assert_ne!(outbox_v2_id, outbox_a_v1_id);
        assert_ne!(outbox_v2_id, legacy_outbox_b_id);
        assert_eq!(
            scalar_string(
                &db,
                "SELECT delivery_status AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_v2_id.as_str()],
            )
            .await
            .as_deref(),
            Some("pending"),
            "new outbox occurrence for {label} must be pending"
        );
    }
    for (outbox_id, label) in [
        (&outbox_a_v1_id, "policy-owned v1"),
        (&legacy_outbox_b_id, "legacy"),
    ] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT delivery_status AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_id.as_str()],
            )
            .await
            .as_deref(),
            Some("suppressed"),
            "{label} outbox must stay suppressed, never resurrected"
        );
    }

    // ===== 原子失败路径：第二个目标当前 due 晚于共同新时间，整个批次必须全回滚 =====
    // 8a. 两个新的来源化 FollowUp（均未到期），C 带一条 legacy pending Outbox；
    //     D 的当前 due（snooze_until + 3600）晚于共同 snooze_until，触发时间校验失败。
    let (_fact_c, follow_up_c) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "batch-snooze-commitment-c",
        "commitment:batch-snooze-c",
        base_due + 7200,
    )
    .await;
    let (_fact_d, follow_up_d) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "batch-snooze-commitment-d",
        "commitment:batch-snooze-d",
        snooze_until + 3600,
    )
    .await;
    let legacy_outbox_c_id = Uuid::new_v4().to_string();
    legacy_pending_outbox_for(
        &db,
        &managed_id,
        &follow_up_c,
        base_due + 7200,
        &legacy_outbox_c_id,
    )
    .await;
    // 8b. 新 OwnerCommand（复用既有 binding，不再插入新 binding，保持恰好一个 active）。
    let command2_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(
                    MessageSource::QqOpenPlatform,
                    &command_account_id,
                    "batch-snooze-cmd-2",
                )
                .unwrap(),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                base_due + 120,
                "推迟另外两条跟进",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let run2_id = ActionRunId::for_owner_command(&command2_event_id, "batch-snooze-v1");
    build_mysql_action_store(db.clone())
        .ensure_action_run(
            &run2_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command2_event_id.clone(),
                command_text: "推迟另外两条跟进".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: base_due + 120,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command2_event_id.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let failing = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(SnoozeFollowUpsPlanner {
            targets: vec![
                FollowUpControlTarget {
                    follow_up_id: FollowUpId::new(follow_up_c.clone()).unwrap(),
                    expected_source_version: 1,
                },
                FollowUpControlTarget {
                    follow_up_id: FollowUpId::new(follow_up_d.clone()).unwrap(),
                    expected_source_version: 1,
                },
            ],
            snooze_until_unix_secs: snooze_until,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(Arc::new(FollowUpControlUseCase::new(
        build_mysql_follow_up_control_store(db.clone()),
    )));
    let run2 = failing
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("second run must be claimed");
    assert!(run2.suspended, "L2 batch snooze must await approval first");
    let checkpoint2_id = run2
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal2_id = run2.proposal_id.expect("suspended run must have proposal");
    assert!(
        failing
            .resume_run(
                &run2_id,
                &checkpoint2_id,
                SecretaryActionResumeInput {
                    proposal_id: proposal2_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command2_event_id.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "target due later than the common snooze time must fail the whole batch"
    );

    // 8c. 全回滚断言：两条 FollowUp 都未变化（仍 scheduled、due/版本原样）、
    //     Outbox 未压制、无审计、无回执、无响应。
    for (id, label) in [(&follow_up_c, "C"), (&follow_up_d, "D")] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await
            .as_deref(),
            Some("scheduled"),
            "follow-up {label} must stay scheduled after atomic failure"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_follow_up_items \
                 WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            1,
            "follow-up {label} version must stay unchanged after atomic failure"
        );
    }
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT due_at_unix_secs AS value FROM secretary_follow_up_items \
             WHERE follow_up_id = ?",
            [follow_up_d.as_str()],
        )
        .await,
        snooze_until + 3600,
        "follow-up D due must stay unchanged after atomic failure"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT delivery_status AS value FROM secretary_notification_outbox \
             WHERE notification_id = ?",
            [legacy_outbox_c_id.as_str()],
        )
        .await
        .as_deref(),
        Some("pending"),
        "outbox must not be suppressed after atomic failure"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id IN (?, ?)",
            [follow_up_c.as_str(), follow_up_d.as_str()],
        )
        .await,
        0,
        "no batch control audit after atomic failure"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run2_id.as_str()],
        )
        .await,
        0,
        "no effect receipt after atomic failure"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run2_id.as_str()],
        )
        .await,
        0,
        "no optimistic success response after atomic failure"
    );
}

// ===== Owner 完成 FollowUp（单条/批量，all-or-nothing + 不再扫描再生） =====

/// 测试用 Planner：固定返回 CompleteFollowUp Proposal，不调用 LLM。
struct CompleteFollowUpPlanner {
    follow_up_id: FollowUpId,
    expected_source_version: u64,
}

#[async_trait]
impl ActionPlannerT for CompleteFollowUpPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::CompleteFollowUp {
                    follow_up_id: self.follow_up_id.clone(),
                    expected_source_version: self.expected_source_version,
                    reason: "Owner 确认该跟进事项已经完成".into(),
                },
                "测试：Owner 审批完成单个 FollowUp",
                Vec::new(),
                Some("complete-follow-up-v1".into()),
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

/// 测试用 Planner：固定返回 CompleteFollowUps Proposal，不调用 LLM。
struct CompleteFollowUpsPlanner {
    targets: Vec<FollowUpControlTarget>,
}

#[async_trait]
impl ActionPlannerT for CompleteFollowUpsPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::CompleteFollowUps {
                    targets: self.targets.clone(),
                    reason: "Owner 确认这些跟进事项都已经完成".into(),
                },
                "测试：Owner 审批批量完成 FollowUp",
                Vec::new(),
                Some("batch-complete-follow-ups-v1".into()),
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

/// policy-owned（candidate 回溯）FollowUp Outbox 的 notification_id。
async fn policy_outbox_for_follow_up(
    db: &sea_orm::DatabaseConnection,
    follow_up_id: &str,
) -> String {
    scalar_string(
        db,
        "SELECT outbox.notification_id AS value \
         FROM secretary_notification_outbox outbox \
         JOIN secretary_notification_candidates candidate \
           ON candidate.notification_candidate_id = outbox.notification_candidate_id \
         WHERE candidate.source_kind = 'follow_up' AND candidate.source_id = ?",
        [follow_up_id],
    )
    .await
    .expect("policy-owned follow-up outbox must exist")
}

/// 断言 FollowUp 已 completed、版本精确为期望值、due 不变。
async fn assert_completed_follow_up(
    db: &sea_orm::DatabaseConnection,
    follow_up_id: &str,
    due_at_unix_secs: i64,
    version: i64,
    label: &str,
) {
    assert_eq!(
        scalar_string(
            db,
            "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
            [follow_up_id],
        )
        .await
        .as_deref(),
        Some("completed"),
        "follow-up {label} must be completed"
    );
    assert_eq!(
        scalar_i64(
            db,
            "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_follow_up_items \
             WHERE follow_up_id = ?",
            [follow_up_id],
        )
        .await,
        version,
        "follow-up {label} version must be exactly {version}"
    );
    assert_eq!(
        scalar_i64(
            db,
            "SELECT CAST(due_at_unix_secs AS SIGNED) AS value FROM secretary_follow_up_items \
             WHERE follow_up_id = ?",
            [follow_up_id],
        )
        .await,
        due_at_unix_secs,
        "follow-up {label} due must stay unchanged"
    );
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn owner_work_control_follow_up_complete_closed_loop_is_atomic_and_no_rescan() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("complete-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();
    let now = SystemClock.now_unix_secs();
    let base_due = now - 3600;

    // 1. 先建来源化 FollowUp（创建托管账号），再建 OwnerCommand 与有效
    //    OwnerBinding —— binding 必须先于策略求值存在（Remind 复验接收方），
    //    但绑定 INSERT 依赖两个账号都已存在。
    let (_fact_a, follow_up_a) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "complete-commitment-a",
        "commitment:complete-a",
        base_due,
    )
    .await;
    let (_fact_c, follow_up_c) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "complete-commitment-c",
        "commitment:complete-c",
        base_due,
    )
    .await;

    // 2. OwnerCommand 与有效 OwnerBinding（两个账号都已存在）。
    let command_account_id = format!("complete-command-{suffix}");
    let command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "complete-cmd-1",
        "完成这条跟进事项",
        base_due + 60,
    )
    .await;
    let command_event = personal_secretary::SourceEventId::new(command_event_id).unwrap();

    // 3. A/C 到期扫描生成 Candidate/Request，统一策略求值生成 policy-owned Outbox。
    let due_report = follow_up_scan_at(&db, base_due).await;
    assert_eq!(due_report.notification_candidates_created, 2);
    assert_eq!(due_report.notification_evaluation_requests_created, 2);
    let policy = NotificationPolicyUseCase::new(
        build_mysql_notification_policy_store(db.clone()),
        Arc::new(SystemClock),
    );
    for _ in 0..2 {
        assert_eq!(
            policy
                .evaluate_next("complete-follow-up-policy", 60, |snapshot| {
                    NotificationPolicyEvaluator
                        .evaluate(&snapshot.evaluation_input(base_due + 1).unwrap())
                })
                .await
                .unwrap(),
            Some(EvaluationCommitResult::Applied)
        );
    }
    let outbox_a_id = policy_outbox_for_follow_up(&db, &follow_up_a).await;
    let outbox_c_id = policy_outbox_for_follow_up(&db, &follow_up_c).await;
    // 4. B 的 fixture（内部扫描在 base_due+7199，不产生新 Candidate）、
    //    A 的 delivered legacy Outbox（完成后必须保留）、B 的 pending legacy Outbox。
    let (_fact_b, follow_up_b) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "complete-commitment-b",
        "commitment:complete-b",
        base_due + 7200,
    )
    .await;
    let delivered_a_id = Uuid::new_v4().to_string();
    let inserted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "INSERT INTO secretary_notification_outbox \
             (notification_id, account_id, follow_up_id, scheduled_at_unix_secs, notification_kind, \
              payload_json, delivery_status) \
             SELECT ?, id, ?, ?, 'owner_reminder', '{}', 'delivered' \
             FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
            vec![
                delivered_a_id.clone().into(),
                follow_up_a.clone().into(),
                base_due.into(),
                MessageSource::NapCat.as_str().into(),
                managed_id.clone().into(),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(
        inserted.rows_affected(),
        1,
        "delivered outbox fixture must insert exactly one row"
    );
    let legacy_b_id = Uuid::new_v4().to_string();
    legacy_pending_outbox_for(
        &db,
        &managed_id,
        &follow_up_b,
        base_due + 7200,
        &legacy_b_id,
    )
    .await;

    // 5. 单条完成 A：Suspend -> 模拟进程重建 -> Resume Approve。
    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&command_event, "complete-v1");
    action_store
        .ensure_action_run(
            &run_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command_event.clone(),
                command_text: "完成这条跟进事项".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: base_due + 60,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command_event.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let control = Arc::new(FollowUpControlUseCase::new(
        build_mysql_follow_up_control_store(db.clone()),
    ));
    let initial = PlannerUseCase::new(
        action_store,
        Arc::new(CompleteFollowUpPlanner {
            follow_up_id: FollowUpId::new(follow_up_a.clone()).unwrap(),
            expected_source_version: 1,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(Arc::clone(&control));
    let run = initial
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("run must be claimed");
    assert!(run.suspended, "L2 complete follow-up must await approval");
    let checkpoint_id = run
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal_id = run.proposal_id.expect("suspended run must have proposal");
    let resumed = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(CompleteFollowUpPlanner {
            follow_up_id: FollowUpId::new(follow_up_a.clone()).unwrap(),
            expected_source_version: 1,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(control);
    let resumed_report = resumed
        .resume_run(
            &run_id,
            &checkpoint_id,
            SecretaryActionResumeInput {
                proposal_id: proposal_id.clone(),
                decision: SecretaryApprovalDecision::Approve,
                command_source_event_id: command_event.clone(),
                approval_source_event_id: None,
            },
        )
        .await
        .expect("approved resume must execute complete effect");
    assert!(
        resumed_report.completed,
        "approved resume must complete the run"
    );

    // 6. 单条完成断言：scheduled -> completed、版本精确 +1、due 不变、
    //    policy-owned Outbox 压制、delivered legacy 保留、审计/回执/响应各一条。
    assert_completed_follow_up(&db, &follow_up_a, base_due, 2, "A").await;
    assert_eq!(
        scalar_string(
            &db,
            "SELECT delivery_status AS value FROM secretary_notification_outbox \
             WHERE notification_id = ?",
            [outbox_a_id.as_str()],
        )
        .await
        .as_deref(),
        Some("suppressed"),
        "policy-owned outbox of A must be suppressed"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT delivery_status AS value FROM secretary_notification_outbox \
             WHERE notification_id = ?",
            [delivered_a_id.as_str()],
        )
        .await
        .as_deref(),
        Some("delivered"),
        "delivered legacy outbox of A must be left untouched"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id = ? AND control_kind = 'complete'",
            [follow_up_a.as_str()],
        )
        .await,
        1,
        "single complete must write one complete-kind audit"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "single complete must persist exactly one effect receipt"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "completed resume must persist one owner response"
    );
    assert!(
        resumed
            .resume_run(
                &run_id,
                &checkpoint_id,
                SecretaryActionResumeInput {
                    proposal_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command_event.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "checkpoint CAS must reject the second approved resume"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id = ?",
            [follow_up_a.as_str()],
        )
        .await,
        1,
        "second resume must not write another audit"
    );

    // 7. 批量完成 B/C：Suspend -> 重启 -> Resume Approve；legacy + policy-owned Outbox。
    let command2_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(
                    MessageSource::QqOpenPlatform,
                    &command_account_id,
                    "complete-cmd-2",
                )
                .unwrap(),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                base_due + 120,
                "把这两条跟进都完成",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let run2_id = ActionRunId::for_owner_command(&command2_event_id, "batch-complete-v1");
    build_mysql_action_store(db.clone())
        .ensure_action_run(
            &run2_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command2_event_id.clone(),
                command_text: "把这两条跟进都完成".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: base_due + 120,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command2_event_id.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let batch = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(CompleteFollowUpsPlanner {
            targets: vec![
                FollowUpControlTarget {
                    follow_up_id: FollowUpId::new(follow_up_b.clone()).unwrap(),
                    expected_source_version: 1,
                },
                FollowUpControlTarget {
                    follow_up_id: FollowUpId::new(follow_up_c.clone()).unwrap(),
                    expected_source_version: 1,
                },
            ],
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(Arc::new(FollowUpControlUseCase::new(
        build_mysql_follow_up_control_store(db.clone()),
    )));
    let run2 = batch
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("batch run must be claimed");
    assert!(run2.suspended, "L2 batch complete must await approval");
    let checkpoint2_id = run2
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal2_id = run2.proposal_id.expect("suspended run must have proposal");
    let batch_report = batch
        .resume_run(
            &run2_id,
            &checkpoint2_id,
            SecretaryActionResumeInput {
                proposal_id: proposal2_id.clone(),
                decision: SecretaryApprovalDecision::Approve,
                command_source_event_id: command2_event_id.clone(),
                approval_source_event_id: None,
            },
        )
        .await
        .expect("approved batch resume must execute complete effect");
    assert!(
        batch_report.completed,
        "approved batch resume must complete the run"
    );
    assert_completed_follow_up(&db, &follow_up_b, base_due + 7200, 2, "B").await;
    assert_completed_follow_up(&db, &follow_up_c, base_due, 2, "C").await;
    for (outbox_id, label) in [(&legacy_b_id, "legacy B"), (&outbox_c_id, "policy-owned C")] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT delivery_status AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_id.as_str()],
            )
            .await
            .as_deref(),
            Some("suppressed"),
            "{label} outbox must be suppressed"
        );
    }
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(DISTINCT effect_id) AS SIGNED) AS value \
             FROM secretary_follow_up_owner_controls WHERE follow_up_id IN (?, ?)",
            [follow_up_b.as_str(), follow_up_c.as_str()],
        )
        .await,
        1,
        "batch audit rows must share one effect_id"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id IN (?, ?)",
            [follow_up_b.as_str(), follow_up_c.as_str()],
        )
        .await,
        2,
        "one audit row per target"
    );
    for (id, label) in [(&follow_up_b, "B"), (&follow_up_c, "C")] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT control_kind AS value FROM secretary_follow_up_owner_controls \
                 WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await
            .as_deref(),
            Some("complete"),
            "audit for {label} must be complete kind"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(previous_source_version AS SIGNED) AS value \
                 FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            1,
            "audit for {label} must record previous version 1"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(current_source_version AS SIGNED) AS value \
                 FROM secretary_follow_up_owner_controls WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            2,
            "audit for {label} must record current version 2"
        );
    }
    let receipt_effect_id = scalar_string(
        &db,
        "SELECT effect_id AS value FROM secretary_action_effect_receipts WHERE run_id = ?",
        [run2_id.as_str()],
    )
    .await
    .expect("batch must persist one effect receipt");
    let audit_effect_id = scalar_exactly_one_string(
        &db,
        "SELECT effect_id AS value FROM secretary_follow_up_owner_controls \
         WHERE follow_up_id = ? AND control_kind = 'complete'",
        [follow_up_b.as_str()],
        "audit row for B must be unique",
    )
    .await;
    assert_eq!(
        receipt_effect_id, audit_effect_id,
        "audit effect_id must match the receipt effect_id"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run2_id.as_str()],
        )
        .await,
        1,
        "batch must persist exactly one effect receipt"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run2_id.as_str()],
        )
        .await,
        1,
        "batch resume must persist one owner response"
    );
    assert!(
        batch
            .resume_run(
                &run2_id,
                &checkpoint2_id,
                SecretaryActionResumeInput {
                    proposal_id: proposal2_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command2_event_id.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "checkpoint CAS must reject the second batch resume"
    );

    // 8. 原子失败路径：D 版本正确、E 版本错误（实际 1、期望 99），整批必须全回滚。
    let (_fact_d, follow_up_d) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "complete-commitment-d",
        "commitment:complete-d",
        base_due + 7200,
    )
    .await;
    let (_fact_e, follow_up_e) = commitment_follow_up_fixture(
        &db,
        &inbound,
        &managed,
        &managed_id,
        "complete-commitment-e",
        "commitment:complete-e",
        base_due + 7200,
    )
    .await;
    let legacy_d_id = Uuid::new_v4().to_string();
    legacy_pending_outbox_for(
        &db,
        &managed_id,
        &follow_up_d,
        base_due + 7200,
        &legacy_d_id,
    )
    .await;
    let command3_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(
                    MessageSource::QqOpenPlatform,
                    &command_account_id,
                    "complete-cmd-3",
                )
                .unwrap(),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                base_due + 180,
                "完成另外两条跟进",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let run3_id = ActionRunId::for_owner_command(&command3_event_id, "batch-complete-fail-v1");
    build_mysql_action_store(db.clone())
        .ensure_action_run(
            &run3_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command3_event_id.clone(),
                command_text: "完成另外两条跟进".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: base_due + 180,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command3_event_id.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let failing = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(CompleteFollowUpsPlanner {
            targets: vec![
                FollowUpControlTarget {
                    follow_up_id: FollowUpId::new(follow_up_d.clone()).unwrap(),
                    expected_source_version: 1,
                },
                FollowUpControlTarget {
                    follow_up_id: FollowUpId::new(follow_up_e.clone()).unwrap(),
                    expected_source_version: 99,
                },
            ],
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_follow_up_control(Arc::new(FollowUpControlUseCase::new(
        build_mysql_follow_up_control_store(db.clone()),
    )));
    let run3 = failing
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("third run must be claimed");
    assert!(
        run3.suspended,
        "L2 batch complete must await approval first"
    );
    let checkpoint3_id = run3
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal3_id = run3.proposal_id.expect("suspended run must have proposal");
    assert!(
        failing
            .resume_run(
                &run3_id,
                &checkpoint3_id,
                SecretaryActionResumeInput {
                    proposal_id: proposal3_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command3_event_id.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "wrong expected_source_version must fail the whole batch"
    );
    for (id, label) in [(&follow_up_d, "D"), (&follow_up_e, "E")] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await
            .as_deref(),
            Some("scheduled"),
            "follow-up {label} must stay scheduled after atomic failure"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_follow_up_items \
                 WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await,
            1,
            "follow-up {label} version must stay unchanged after atomic failure"
        );
    }
    assert_eq!(
        scalar_string(
            &db,
            "SELECT delivery_status AS value FROM secretary_notification_outbox \
             WHERE notification_id = ?",
            [legacy_d_id.as_str()],
        )
        .await
        .as_deref(),
        Some("pending"),
        "outbox must not be suppressed after atomic failure"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_owner_controls \
             WHERE follow_up_id IN (?, ?)",
            [follow_up_d.as_str(), follow_up_e.as_str()],
        )
        .await,
        0,
        "no batch control audit after atomic failure"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run3_id.as_str()],
        )
        .await,
        0,
        "no effect receipt after atomic failure"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run3_id.as_str()],
        )
        .await,
        0,
        "no optimistic success response after atomic failure"
    );

    // 9. 后续扫描不重新生成：A/B/C 的 candidate/outbox 计数不再增加，
    //    follow_up_items 每个来源事实仍只有一行（INSERT IGNORE 去重）。
    follow_up_scan_at(&db, base_due + 14400).await;
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_notification_candidates \
             WHERE source_kind = 'follow_up' AND source_id IN (?, ?, ?)",
            [
                follow_up_a.as_str(),
                follow_up_b.as_str(),
                follow_up_c.as_str()
            ],
        )
        .await,
        2,
        "only A and C keep their v1 candidates; no new candidates for completed items"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_notification_outbox outbox \
             JOIN secretary_notification_candidates candidate \
               ON candidate.notification_candidate_id = outbox.notification_candidate_id \
             WHERE candidate.source_kind = 'follow_up' AND candidate.source_id IN (?, ?, ?)",
            [
                follow_up_a.as_str(),
                follow_up_b.as_str(),
                follow_up_c.as_str()
            ],
        )
        .await,
        2,
        "no new policy-owned outbox rows for completed items"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_items \
             WHERE follow_up_id IN (?, ?, ?)",
            [
                follow_up_a.as_str(),
                follow_up_b.as_str(),
                follow_up_c.as_str()
            ],
        )
        .await,
        3,
        "completed items must not be re-materialized by later scans"
    );
    for (id, label) in [
        (&follow_up_a, "A"),
        (&follow_up_b, "B"),
        (&follow_up_c, "C"),
    ] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT status AS value FROM secretary_follow_up_items WHERE follow_up_id = ?",
                [id.as_str()],
            )
            .await
            .as_deref(),
            Some("completed"),
            "follow-up {label} must stay completed after later scans"
        );
    }
}

// ===== Owner 关闭 ResponseExpectation（单条/批量，all-or-nothing + 不再扫描再生） =====

/// 测试用 Planner：固定返回 DismissResponseExpectation Proposal，不调用 LLM。
struct DismissResponseExpectationPlanner {
    expectation_id: ResponseExpectationId,
    expected_source_version: u64,
}

#[async_trait]
impl ActionPlannerT for DismissResponseExpectationPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::DismissResponseExpectation {
                    expectation_id: self.expectation_id.clone(),
                    expected_source_version: self.expected_source_version,
                    reason: "Owner 确认不需要继续提醒回复".into(),
                },
                "测试：Owner 审批关闭单个回复期待",
                Vec::new(),
                Some("dismiss-response-expectation-v1".into()),
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

/// 测试用 Planner：固定返回 DismissResponseExpectations Proposal，不调用 LLM。
struct DismissResponseExpectationsPlanner {
    targets: Vec<ResponseExpectationControlTarget>,
}

#[async_trait]
impl ActionPlannerT for DismissResponseExpectationsPlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::DismissResponseExpectations {
                    targets: self.targets.clone(),
                    reason: "Owner 确认这些回复期待都不需要继续提醒".into(),
                },
                "测试：Owner 审批批量关闭回复期待",
                Vec::new(),
                Some("batch-dismiss-response-expectations-v1".into()),
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

/// 创建外部联系人开放问题：来源消息 + 开放线程 + open 问题 + 问题来源。
/// 返回 (thread_id, question_id)，随后由期望扫描物化为回复期待。
async fn open_question_fixture(
    db: &sea_orm::DatabaseConnection,
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    managed_id: &str,
    message_id: &str,
    question_text: &str,
    occurred_at_unix_secs: i64,
) -> (String, String) {
    let question_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::NapCat, managed_id, message_id).unwrap(),
                ConversationRef::new(ConversationKind::Group, "expect-group").unwrap(),
                VerifiedActor::new(VerifiedActorKind::External, "customer").unwrap(),
                occurred_at_unix_secs,
                question_text,
                Vec::new(),
            )
            .unwrap(),
        )
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
         VALUES (?, ?, 'napcat', ?, 'customer', ?, 'open', 9500)",
        [
            question_id.clone().into(),
            thread_id.clone().into(),
            managed_id.to_owned().into(),
            question_text.to_owned().into(),
        ],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_thread_question_sources (question_id, source_event_id) \
         VALUES (?, ?)",
        [
            question_id.clone().into(),
            question_event_id.as_str().into(),
        ],
    ))
    .await
    .unwrap();
    (thread_id, question_id)
}

/// 查询问题对应的回复期待 ID。
async fn expectation_id_for_question(
    db: &sea_orm::DatabaseConnection,
    question_id: &str,
) -> String {
    scalar_string(
        db,
        "SELECT expectation_id AS value FROM secretary_response_expectations \
         WHERE source_question_id = ?",
        [question_id],
    )
    .await
    .expect("expectation must exist for question")
}

/// policy-owned（candidate 回溯）回复期待 Outbox 的 notification_id。
async fn expectation_outbox_id(db: &sea_orm::DatabaseConnection, expectation_id: &str) -> String {
    scalar_string(
        db,
        "SELECT outbox.notification_id AS value \
         FROM secretary_notification_outbox outbox \
         JOIN secretary_notification_candidates candidate \
           ON candidate.notification_candidate_id = outbox.notification_candidate_id \
         WHERE candidate.source_kind = 'response_expectation' AND candidate.source_id = ?",
        [expectation_id],
    )
    .await
    .expect("policy-owned expectation outbox must exist")
}

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn owner_work_control_response_expectation_dismiss_closed_loop_is_atomic_and_no_rescan() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("expect-dismiss-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();
    let now = SystemClock.now_unix_secs();
    // 问题发生在 30_000 秒前，回复超时 14_400 秒 => due = occurred + 14_400 <= now。
    let q_time = now - 30_000;
    let expectation_due = q_time + 14_400;

    // 1. 先建开放问题（创建托管账号），再建 OwnerCommand 与有效 OwnerBinding
    //    —— binding 必须先于策略求值存在，但绑定 INSERT 依赖两个账号都已存在。
    let (_thread_1, question_1) = open_question_fixture(
        &db,
        &inbound,
        &managed_id,
        "expect-question-1",
        "报价单今天能发给我吗？",
        q_time,
    )
    .await;
    let (_thread_2, question_2) = open_question_fixture(
        &db,
        &inbound,
        &managed_id,
        "expect-question-2",
        "会议纪要在哪里看？",
        q_time,
    )
    .await;
    let (_thread_3, question_3) = open_question_fixture(
        &db,
        &inbound,
        &managed_id,
        "expect-question-3",
        "这份合同还需要修改吗？",
        q_time,
    )
    .await;
    // 2. OwnerCommand 与有效 OwnerBinding（两个账号都已存在），先于策略求值。
    let command_account_id = format!("expect-dismiss-command-{suffix}");
    let command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "expect-cmd-1",
        "关闭这条回复期待",
        now - 20_000,
    )
    .await;
    let command_event = personal_secretary::SourceEventId::new(command_event_id).unwrap();
    let report = follow_up_scan_at(&db, now).await;
    assert_eq!(report.response_expectations_materialized, 3);
    assert_eq!(report.notification_candidates_created, 3);
    assert_eq!(report.notification_evaluation_requests_created, 3);
    let expectation_1 = expectation_id_for_question(&db, &question_1).await;
    let expectation_2 = expectation_id_for_question(&db, &question_2).await;
    let expectation_3 = expectation_id_for_question(&db, &question_3).await;
    let policy = NotificationPolicyUseCase::new(
        build_mysql_notification_policy_store(db.clone()),
        Arc::new(SystemClock),
    );
    for _ in 0..3 {
        assert_eq!(
            policy
                .evaluate_next("expect-dismiss-policy", 60, |snapshot| {
                    NotificationPolicyEvaluator
                        .evaluate(&snapshot.evaluation_input(now + 1).unwrap())
                })
                .await
                .unwrap(),
            Some(EvaluationCommitResult::Applied)
        );
    }
    let outbox_1 = expectation_outbox_id(&db, &expectation_1).await;
    let outbox_2 = expectation_outbox_id(&db, &expectation_2).await;
    let outbox_3 = expectation_outbox_id(&db, &expectation_3).await;

    // 3. 单条关闭 E1：Suspend -> 模拟进程重建 -> Resume Approve。
    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&command_event, "dismiss-expect-v1");
    action_store
        .ensure_action_run(
            &run_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command_event.clone(),
                command_text: "关闭这条回复期待".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: now - 20_000,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command_event.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let control = Arc::new(ResponseExpectationControlUseCase::new(
        build_mysql_response_expectation_control_store(db.clone()),
    ));
    let initial = PlannerUseCase::new(
        action_store,
        Arc::new(DismissResponseExpectationPlanner {
            expectation_id: ResponseExpectationId::new(expectation_1.clone()).unwrap(),
            expected_source_version: 1,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_response_expectation_control(Arc::clone(&control));
    let run = initial
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("run must be claimed");
    assert!(run.suspended, "L2 dismiss expectation must await approval");
    let checkpoint_id = run
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal_id = run.proposal_id.expect("suspended run must have proposal");
    let resumed = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(DismissResponseExpectationPlanner {
            expectation_id: ResponseExpectationId::new(expectation_1.clone()).unwrap(),
            expected_source_version: 1,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_response_expectation_control(control);
    let resumed_report = resumed
        .resume_run(
            &run_id,
            &checkpoint_id,
            SecretaryActionResumeInput {
                proposal_id: proposal_id.clone(),
                decision: SecretaryApprovalDecision::Approve,
                command_source_event_id: command_event.clone(),
                approval_source_event_id: None,
            },
        )
        .await
        .expect("approved resume must execute dismiss effect");
    assert!(
        resumed_report.completed,
        "approved resume must complete the run"
    );

    // 4. 单条关闭断言：active -> dismissed、版本精确 +1、due 不变、
    //    OpenQuestion 仍 open、Thread 未被关闭、Outbox 压制、审计/回执/响应各一条。
    assert_dismissed_expectation(&db, &expectation_1, expectation_due, &question_1, 2, "E1").await;
    assert_eq!(
        scalar_string(
            &db,
            "SELECT delivery_status AS value FROM secretary_notification_outbox \
             WHERE notification_id = ?",
            [outbox_1.as_str()],
        )
        .await
        .as_deref(),
        Some("suppressed"),
        "outbox of E1 must be suppressed"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value \
             FROM secretary_response_expectation_owner_controls WHERE expectation_id = ?",
            [expectation_1.as_str()],
        )
        .await,
        1,
        "single dismiss must write exactly one immutable control audit"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT previous_status AS value \
             FROM secretary_response_expectation_owner_controls WHERE expectation_id = ?",
            [expectation_1.as_str()],
        )
        .await
        .as_deref(),
        Some("active"),
        "audit for E1 must record previous status active"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT current_status AS value \
             FROM secretary_response_expectation_owner_controls WHERE expectation_id = ?",
            [expectation_1.as_str()],
        )
        .await
        .as_deref(),
        Some("dismissed"),
        "audit for E1 must record current status dismissed"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "single dismiss must persist exactly one effect receipt"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "completed resume must persist one owner response"
    );
    assert!(
        resumed
            .resume_run(
                &run_id,
                &checkpoint_id,
                SecretaryActionResumeInput {
                    proposal_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command_event.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "checkpoint CAS must reject the second approved resume"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value \
             FROM secretary_response_expectation_owner_controls WHERE expectation_id = ?",
            [expectation_1.as_str()],
        )
        .await,
        1,
        "second resume must not write another audit"
    );

    // 5. 批量关闭 E2/E3：Suspend -> 重启 -> Resume Approve。
    let command2_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(
                    MessageSource::QqOpenPlatform,
                    &command_account_id,
                    "expect-cmd-2",
                )
                .unwrap(),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                now - 10_000,
                "把这两条回复期待都关闭",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let run2_id = ActionRunId::for_owner_command(&command2_event_id, "batch-dismiss-expect-v1");
    build_mysql_action_store(db.clone())
        .ensure_action_run(
            &run2_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command2_event_id.clone(),
                command_text: "把这两条回复期待都关闭".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: now - 10_000,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command2_event_id.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let batch = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(DismissResponseExpectationsPlanner {
            targets: vec![
                ResponseExpectationControlTarget {
                    expectation_id: ResponseExpectationId::new(expectation_2.clone()).unwrap(),
                    expected_source_version: 1,
                },
                ResponseExpectationControlTarget {
                    expectation_id: ResponseExpectationId::new(expectation_3.clone()).unwrap(),
                    expected_source_version: 1,
                },
            ],
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_response_expectation_control(Arc::new(ResponseExpectationControlUseCase::new(
        build_mysql_response_expectation_control_store(db.clone()),
    )));
    let run2 = batch
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("batch run must be claimed");
    assert!(run2.suspended, "L2 batch dismiss must await approval");
    let checkpoint2_id = run2
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal2_id = run2.proposal_id.expect("suspended run must have proposal");
    let batch_report = batch
        .resume_run(
            &run2_id,
            &checkpoint2_id,
            SecretaryActionResumeInput {
                proposal_id: proposal2_id.clone(),
                decision: SecretaryApprovalDecision::Approve,
                command_source_event_id: command2_event_id.clone(),
                approval_source_event_id: None,
            },
        )
        .await
        .expect("approved batch resume must execute dismiss effect");
    assert!(
        batch_report.completed,
        "approved batch resume must complete the run"
    );
    assert_dismissed_expectation(&db, &expectation_2, expectation_due, &question_2, 2, "E2").await;
    assert_dismissed_expectation(&db, &expectation_3, expectation_due, &question_3, 2, "E3").await;
    for (outbox_id, label) in [(&outbox_2, "E2"), (&outbox_3, "E3")] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT delivery_status AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_id.as_str()],
            )
            .await
            .as_deref(),
            Some("suppressed"),
            "outbox of {label} must be suppressed"
        );
    }
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(DISTINCT effect_id) AS SIGNED) AS value \
             FROM secretary_response_expectation_owner_controls \
             WHERE expectation_id IN (?, ?)",
            [expectation_2.as_str(), expectation_3.as_str()],
        )
        .await,
        1,
        "batch audit rows must share one effect_id"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value \
             FROM secretary_response_expectation_owner_controls \
             WHERE expectation_id IN (?, ?)",
            [expectation_2.as_str(), expectation_3.as_str()],
        )
        .await,
        2,
        "one audit row per target"
    );
    let receipt_effect_id = scalar_string(
        &db,
        "SELECT effect_id AS value FROM secretary_action_effect_receipts WHERE run_id = ?",
        [run2_id.as_str()],
    )
    .await
    .expect("batch must persist one effect receipt");
    let audit_effect_id = scalar_exactly_one_string(
        &db,
        "SELECT effect_id AS value FROM secretary_response_expectation_owner_controls \
         WHERE expectation_id = ?",
        [expectation_2.as_str()],
        "audit row for E2 must be unique",
    )
    .await;
    assert_eq!(
        receipt_effect_id, audit_effect_id,
        "audit effect_id must match the receipt effect_id"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run2_id.as_str()],
        )
        .await,
        1,
        "batch must persist exactly one effect receipt"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run2_id.as_str()],
        )
        .await,
        1,
        "batch resume must persist one owner response"
    );
    assert!(
        batch
            .resume_run(
                &run2_id,
                &checkpoint2_id,
                SecretaryActionResumeInput {
                    proposal_id: proposal2_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command2_event_id.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "checkpoint CAS must reject the second batch resume"
    );

    // 6. 原子失败路径：E4 版本正确、E5 版本错误（实际 1、期望 99），整批全回滚。
    //    新问题在成功路径断言之后创建，避免污染前面的 candidate/outbox 计数。
    let (_thread_4, question_4) = open_question_fixture(
        &db,
        &inbound,
        &managed_id,
        "expect-question-4",
        "周五的会议还开吗？",
        q_time,
    )
    .await;
    let (_thread_5, question_5) = open_question_fixture(
        &db,
        &inbound,
        &managed_id,
        "expect-question-5",
        "什么时候能给我报价？",
        q_time,
    )
    .await;
    let report4 = follow_up_scan_at(&db, now).await;
    assert_eq!(report4.response_expectations_materialized, 2);
    let expectation_4 = expectation_id_for_question(&db, &question_4).await;
    let expectation_5 = expectation_id_for_question(&db, &question_5).await;
    for _ in 0..2 {
        assert_eq!(
            policy
                .evaluate_next("expect-dismiss-policy", 60, |snapshot| {
                    NotificationPolicyEvaluator
                        .evaluate(&snapshot.evaluation_input(now + 1).unwrap())
                })
                .await
                .unwrap(),
            Some(EvaluationCommitResult::Applied)
        );
    }
    let outbox_4 = expectation_outbox_id(&db, &expectation_4).await;
    let outbox_5 = expectation_outbox_id(&db, &expectation_5).await;
    let command3_event_id = inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(
                    MessageSource::QqOpenPlatform,
                    &command_account_id,
                    "expect-cmd-3",
                )
                .unwrap(),
                ConversationRef::new(ConversationKind::OwnerControl, "owner-conv").unwrap(),
                VerifiedActor::new(VerifiedActorKind::Owner, "owner-openid").unwrap(),
                now - 5_000,
                "关闭另外两条回复期待",
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .clone();
    let run3_id =
        ActionRunId::for_owner_command(&command3_event_id, "batch-dismiss-expect-fail-v1");
    build_mysql_action_store(db.clone())
        .ensure_action_run(
            &run3_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command3_event_id.clone(),
                command_text: "关闭另外两条回复期待".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: now - 5_000,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command3_event_id.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let failing = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(DismissResponseExpectationsPlanner {
            targets: vec![
                ResponseExpectationControlTarget {
                    expectation_id: ResponseExpectationId::new(expectation_4.clone()).unwrap(),
                    expected_source_version: 1,
                },
                ResponseExpectationControlTarget {
                    expectation_id: ResponseExpectationId::new(expectation_5.clone()).unwrap(),
                    expected_source_version: 99,
                },
            ],
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_response_expectation_control(Arc::new(ResponseExpectationControlUseCase::new(
        build_mysql_response_expectation_control_store(db.clone()),
    )));
    let run3 = failing
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("third run must be claimed");
    assert!(run3.suspended, "L2 batch dismiss must await approval first");
    let checkpoint3_id = run3
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal3_id = run3.proposal_id.expect("suspended run must have proposal");
    assert!(
        failing
            .resume_run(
                &run3_id,
                &checkpoint3_id,
                SecretaryActionResumeInput {
                    proposal_id: proposal3_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command3_event_id.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "wrong expected_source_version must fail the whole batch"
    );
    for (id, label) in [(&expectation_4, "E4"), (&expectation_5, "E5")] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT expectation_status AS value FROM secretary_response_expectations \
                 WHERE expectation_id = ?",
                [id.as_str()],
            )
            .await
            .as_deref(),
            Some("active"),
            "expectation {label} must stay active after atomic failure"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(source_version AS SIGNED) AS value \
                 FROM secretary_response_expectations WHERE expectation_id = ?",
                [id.as_str()],
            )
            .await,
            1,
            "expectation {label} version must stay unchanged after atomic failure"
        );
    }
    for (outbox_id, label) in [(&outbox_4, "E4"), (&outbox_5, "E5")] {
        assert_eq!(
            scalar_string(
                &db,
                "SELECT delivery_status AS value FROM secretary_notification_outbox \
                 WHERE notification_id = ?",
                [outbox_id.as_str()],
            )
            .await
            .as_deref(),
            Some("pending"),
            "outbox of {label} must not be suppressed after atomic failure"
        );
    }
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value \
             FROM secretary_response_expectation_owner_controls \
             WHERE expectation_id IN (?, ?)",
            [expectation_4.as_str(), expectation_5.as_str()],
        )
        .await,
        0,
        "no batch control audit after atomic failure"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run3_id.as_str()],
        )
        .await,
        0,
        "no effect receipt after atomic failure"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run3_id.as_str()],
        )
        .await,
        0,
        "no optimistic success response after atomic failure"
    );

    // 7. 后续扫描不重新创建：E1/E2/E3 保持 dismissed（不改成 resolved）、
    //    candidate/outbox 计数不增加、问题与线程保持 open。
    follow_up_scan_at(&db, now + 3600).await;
    for (id, question_id, label) in [
        (&expectation_1, &question_1, "E1"),
        (&expectation_2, &question_2, "E2"),
        (&expectation_3, &question_3, "E3"),
    ] {
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(COUNT(*) AS SIGNED) AS value \
                 FROM secretary_response_expectations WHERE source_question_id = ?",
                [question_id.as_str()],
            )
            .await,
            1,
            "expectation {label} must not be re-materialized"
        );
        assert_eq!(
            scalar_string(
                &db,
                "SELECT expectation_status AS value FROM secretary_response_expectations \
                 WHERE expectation_id = ?",
                [id.as_str()],
            )
            .await
            .as_deref(),
            Some("dismissed"),
            "expectation {label} must stay dismissed, not rewritten to resolved"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_notification_candidates \
                 WHERE source_kind = 'response_expectation' AND source_id = ?",
                [id.as_str()],
            )
            .await,
            1,
            "expectation {label} must keep exactly its v1 candidate"
        );
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_notification_outbox \
                 WHERE notification_candidate_id IN (SELECT notification_candidate_id \
                   FROM secretary_notification_candidates \
                   WHERE source_kind = 'response_expectation' AND source_id = ?)",
                [id.as_str()],
            )
            .await,
            1,
            "expectation {label} must not get a new outbox row"
        );
        assert_eq!(
            scalar_string(
                &db,
                "SELECT status AS value FROM secretary_thread_open_questions \
                 WHERE question_id = ?",
                [question_id.as_str()],
            )
            .await
            .as_deref(),
            Some("open"),
            "open question of {label} must stay open"
        );
    }
}

/// 断言回复期待已 dismissed、版本精确为期望值、due 不变，且其开放问题仍 open。
async fn assert_dismissed_expectation(
    db: &sea_orm::DatabaseConnection,
    expectation_id: &str,
    due_at_unix_secs: i64,
    question_id: &str,
    version: i64,
    label: &str,
) {
    assert_eq!(
        scalar_string(
            db,
            "SELECT expectation_status AS value FROM secretary_response_expectations \
             WHERE expectation_id = ?",
            [expectation_id],
        )
        .await
        .as_deref(),
        Some("dismissed"),
        "expectation {label} must be dismissed"
    );
    assert_eq!(
        scalar_i64(
            db,
            "SELECT CAST(source_version AS SIGNED) AS value FROM secretary_response_expectations \
             WHERE expectation_id = ?",
            [expectation_id],
        )
        .await,
        version,
        "expectation {label} version must be exactly {version}"
    );
    assert_eq!(
        scalar_i64(
            db,
            "SELECT CAST(due_at_unix_secs AS SIGNED) AS value \
             FROM secretary_response_expectations WHERE expectation_id = ?",
            [expectation_id],
        )
        .await,
        due_at_unix_secs,
        "expectation {label} due must stay unchanged"
    );
    assert_eq!(
        scalar_string(
            db,
            "SELECT status AS value FROM secretary_thread_open_questions \
             WHERE question_id = ?",
            [question_id],
        )
        .await
        .as_deref(),
        Some("open"),
        "open question of {label} must stay open after dismissal"
    );
    assert_eq!(
        scalar_string(
            db,
            "SELECT status AS value FROM secretary_event_threads WHERE thread_id = \
               (SELECT thread_id FROM secretary_thread_open_questions WHERE question_id = ?)",
            [question_id],
        )
        .await
        .as_deref(),
        Some("open"),
        "thread of {label} must not be closed by dismissal"
    );
}
// ===== 结构化记忆候选生产与 Owner 审批闭环（MySQL 集成） =====

/// 测试提取器：确定性产出三类候选（person/project/commitment），
/// 承诺固定 due=200_000（在 follow_up horizon 内，验证 FollowUp 只生成一个）。
/// 每次输入相同事件都产出相同候选，靠 fingerprint 幂等去重验证"重复扫描不重复"。
struct FakeCandidateExtractor {
    extractor_version: String,
}

#[async_trait]
impl MemoryCandidateExtractorT for FakeCandidateExtractor {
    async fn extract(
        &self,
        batch: &MemoryCandidateBatch,
    ) -> Result<Vec<MemoryCandidate>, MemoryCandidateExtractorError> {
        let mut candidates = Vec::new();
        for event in batch.events.iter().filter(|event| !event.content_omitted) {
            let text = event.normalized_text.trim();
            let sources = vec![MemoryCandidateSource {
                source_event_id: event.source_event_id.clone(),
                actor: event.actor.clone(),
                occurred_at_unix_secs: event.occurred_at_unix_secs,
                content_trust_level: event.content_trust_level,
            }];
            if let Some(rest) = text.strip_prefix("人物：") {
                let payload = MemoryPayload::Person(PersonMemory {
                    person: event.actor.clone(),
                    relationship: Some(rest.to_owned()),
                    responsibilities: Vec::new(),
                    communication_preferences: Vec::new(),
                });
                let subject = format!("person:{}", event.actor.actor_id);
                candidates.push(build_fake_candidate(
                    batch,
                    subject,
                    payload,
                    &sources,
                    &self.extractor_version,
                ));
            } else if let Some(rest) = text.strip_prefix("项目：") {
                let (key, goal) = match rest.find(char::is_whitespace) {
                    Some(index) => (&rest[..index], rest[index..].trim()),
                    None => (rest, ""),
                };
                if !key.is_empty() {
                    let payload = MemoryPayload::Project(ProjectMemory {
                        project_key: key.to_owned(),
                        goal: goal.to_owned(),
                        member_actor_ids: Vec::new(),
                        progress: None,
                        decision_ids: Vec::new(),
                        risks: Vec::new(),
                        blockers: Vec::new(),
                        artifact_refs: Vec::new(),
                    });
                    let subject = format!("project:{key}");
                    candidates.push(build_fake_candidate(
                        batch,
                        subject,
                        payload,
                        &sources,
                        &self.extractor_version,
                    ));
                }
            } else if let Some(rest) = text.strip_prefix("承诺：") {
                // 兼容"承诺：给 X action"与"承诺：X action"两种表述。
                let rest = rest.strip_prefix("给").map(str::trim).unwrap_or(rest);
                let (beneficiary_id, action) = match rest.find(char::is_whitespace) {
                    Some(index) => (&rest[..index], rest[index..].trim()),
                    None => (rest, ""),
                };
                if !action.is_empty()
                    && let Some(beneficiary) = batch
                        .events
                        .iter()
                        .find(|batch_event| batch_event.actor.actor_id == beneficiary_id)
                {
                    // 承诺双方事件都必须进入证据来源（身份-证据强绑定）。
                    let mut commitment_sources = sources.clone();
                    if beneficiary.source_event_id != event.source_event_id {
                        commitment_sources.push(MemoryCandidateSource {
                            source_event_id: beneficiary.source_event_id.clone(),
                            actor: beneficiary.actor.clone(),
                            occurred_at_unix_secs: beneficiary.occurred_at_unix_secs,
                            content_trust_level: beneficiary.content_trust_level,
                        });
                    }
                    let payload = MemoryPayload::Commitment(CommitmentMemory {
                        promisor: event.actor.clone(),
                        beneficiary: beneficiary.actor.clone(),
                        action: action.to_owned(),
                        due_at_unix_secs: Some(200_000),
                        status: CommitmentStatus::Proposed,
                        completion_source_event_id: None,
                    });
                    let subject = format!(
                        "commitment:{}:{}:{}",
                        event.actor.actor_id,
                        beneficiary.actor.actor_id,
                        action.chars().take(160).collect::<String>()
                    );
                    candidates.push(build_fake_candidate(
                        batch,
                        subject,
                        payload,
                        &commitment_sources,
                        &self.extractor_version,
                    ));
                }
            }
        }
        Ok(candidates)
    }
}

/// 构造 proposed/version 1 候选；fingerprint 必须由领域函数派生，
/// 否则 validate_memory_candidate 会在提交时拒绝（防止提取器伪造）。
fn build_fake_candidate(
    batch: &MemoryCandidateBatch,
    subject_key: String,
    payload: MemoryPayload,
    sources: &[MemoryCandidateSource],
    extractor_version: &str,
) -> MemoryCandidate {
    let fingerprint = candidate_fingerprint(
        &batch.account,
        &payload,
        &subject_key,
        sources,
        extractor_version,
    );
    MemoryCandidate {
        candidate_id: MemoryCandidateId::generate(),
        account: batch.account.clone(),
        subject_key,
        payload,
        status: MemoryCandidateStatus::Proposed,
        version: MemoryCandidateVersion::new(INITIAL_CANDIDATE_VERSION)
            .expect("initial candidate version is a valid constant"),
        extractor_version: extractor_version.to_owned(),
        deterministic_fingerprint: fingerprint,
        sources: sources.to_vec(),
    }
}

/// 插入一条 normal 内容信任的候选来源事件，返回 source_event_id。
async fn insert_candidate_source_event(
    inbound: &Arc<dyn personal_secretary::PersonalSecretaryStoreT>,
    managed_id: &str,
    message_id: &str,
    conversation_name: &str,
    actor_id: &str,
    text: &str,
    occurred_at_unix_secs: i64,
) -> String {
    inbound
        .insert_message_if_absent(
            &InboundMessageEnvelope::new(
                SourceMessageRef::new(MessageSource::NapCat, managed_id, message_id).unwrap(),
                ConversationRef::new(ConversationKind::Group, conversation_name).unwrap(),
                VerifiedActor::new(VerifiedActorKind::External, actor_id).unwrap(),
                occurred_at_unix_secs,
                text,
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .source_event_id()
        .as_str()
        .to_owned()
}

/// Planner 固定输出 ApproveMemoryCandidate 提案。
struct ApproveMemoryCandidatePlanner {
    candidate_id: MemoryCandidateId,
    expected_version: u64,
}

#[async_trait]
impl ActionPlannerT for ApproveMemoryCandidatePlanner {
    async fn plan(&self, _input: &PlannerInput) -> Result<PlannerOutput, PlannerError> {
        Ok(PlannerOutput::Proposal(
            SecretaryActionProposal::new(
                SecretaryAction::ApproveMemoryCandidate {
                    candidate_id: self.candidate_id.clone(),
                    expected_candidate_version: self.expected_version,
                    reason: "Owner 确认该候选值得长期记忆".into(),
                },
                "测试：Owner 批准记忆候选",
                Vec::new(),
                Some("approve-memory-candidate-v1".into()),
            )
            .map_err(|error| PlannerError::InvalidOutput(error.to_string()))?,
        ))
    }
}

/// 统计指定事实物化出的 follow_up 行数（scoped 到目标事实，避免全库计数污染）。
async fn follow_up_count_for_fact(db: &sea_orm::DatabaseConnection, fact_id: &str) -> i64 {
    scalar_i64(
        db,
        "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_follow_up_items \
         WHERE source_memory_fact_id = ?",
        [fact_id],
    )
    .await
}

/// 创建 OwnerCommand 的 action_run 并领取，返回 (run_id, lease_token)。
async fn claim_control_run(
    action_store: &Arc<dyn personal_secretary::ActionStoreT>,
    command_event_id: &SourceEventId,
    account: &SourceAccountRef,
) -> (ActionRunId, ActionLeaseToken) {
    let run_id = ActionRunId::for_owner_command(command_event_id, "candidate-control-v1");
    action_store
        .ensure_action_run(
            &run_id,
            &ActionRunSeed {
                account: account.clone(),
                command_source_event_id: command_event_id.clone(),
                command_text: "审批记忆候选".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: 100_100,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command_event_id.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let claimed = action_store
        .claim_pending_run("test-worker", 60, 100_100)
        .await
        .unwrap()
        .expect("run must be claimable");
    (claimed.run_id, claimed.lease_token)
}

/// 直接调用控制用例（单事务边界），返回存储错误以便精确断言负路径。
async fn candidate_control_apply(
    control: &MemoryCandidateControlUseCase,
    account: &SourceAccountRef,
    command_event_id: &str,
    run_id: &ActionRunId,
    lease_token: &ActionLeaseToken,
    effect_tag: &str,
    action: SecretaryAction,
) -> Result<personal_secretary::SecretaryActionReceipt, MemoryCandidateControlStoreError> {
    let proposal = SecretaryActionProposal::new(
        action.clone(),
        "测试：控制用例负路径",
        Vec::new(),
        Some(format!("idem-{effect_tag}")),
    )
    .map_err(|error| MemoryCandidateControlStoreError::InvalidData(error.to_string()))?;
    let proposal_json = serde_json::to_string(&proposal)
        .map_err(|error| MemoryCandidateControlStoreError::InvalidData(error.to_string()))?;
    control
        .apply_effect(&MemoryCandidateControlEffectRequest {
            account: account.clone(),
            command_source_event_id: SourceEventId::new(command_event_id).unwrap(),
            run_id: run_id.clone(),
            lease_token: lease_token.clone(),
            effect_id: format!("mc-effect-{effect_tag}"),
            proposal_id: proposal.proposal_id.clone(),
            proposal_json,
            action,
        })
        .await
}

/// 插入一条 proposed 候选与精确来源（绕过提取器，聚焦审批事务的负路径）。
#[allow(clippy::too_many_arguments)]
async fn insert_candidate_row(
    db: &sea_orm::DatabaseConnection,
    account: &SourceAccountRef,
    candidate_id: &str,
    kind: &str,
    subject_key: &str,
    payload: MemoryPayload,
    source_event_id: &str,
    source_actor: &str,
) {
    let payload_json = serde_json::to_string(&payload).unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_memory_candidates \
         (candidate_id, account_id, candidate_kind, subject_key, payload_json, candidate_status, \
          candidate_version, extractor_version, deterministic_fingerprint) \
         SELECT ?, id, ?, ?, ?, 'proposed', 1, 'v1', ? \
         FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
        vec![
            candidate_id.to_owned().into(),
            kind.to_owned().into(),
            subject_key.to_owned().into(),
            payload_json.into(),
            format!("fp-{candidate_id}").into(),
            account.channel.as_str().into(),
            account.account_id.clone().into(),
        ],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_memory_candidate_sources \
         (candidate_id, source_event_id, account_id, actor_platform_id, content_trust_level, \
          occurred_at_unix_secs) \
         SELECT ?, ?, id, ?, 'normal', 100000 \
         FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
        vec![
            candidate_id.to_owned().into(),
            source_event_id.to_owned().into(),
            source_actor.to_owned().into(),
            account.channel.as_str().into(),
            account.account_id.clone().into(),
        ],
    ))
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn memory_candidate_owner_approval_loop_is_exact_once_through_suspend_resume() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("candidate-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();

    // 1. 三条 normal 来源事件（人物/项目/承诺各一）。批次按会话分界（claim 时
    //    同一批次只取一个 conversation），因此三条事件放在同一会话内，承诺的
    //    受益人 bob 才能在同一批内解析；跨会话拼接由提取器拒绝（见下方注释）。
    insert_candidate_source_event(
        &inbound,
        &managed_id,
        "cand-e1",
        "cand-group",
        "alice",
        "人物：alice 是我本科同学",
        100_000,
    )
    .await;
    insert_candidate_source_event(
        &inbound,
        &managed_id,
        "cand-e2",
        "cand-group",
        "bob",
        "项目：alpha 完成数据库迁移",
        100_001,
    )
    .await;
    insert_candidate_source_event(
        &inbound,
        &managed_id,
        "cand-e3",
        "cand-group",
        "alice",
        "承诺：给 bob 发送报价单",
        100_002,
    )
    .await;

    // 2. 提取：Fake 提取器产出三类候选并提交
    let candidate_store = build_mysql_memory_candidate_store(db.clone());
    let candidate_use_case = Arc::new(
        MemoryCandidateUseCase::new(
            candidate_store,
            Arc::new(FakeCandidateExtractor {
                extractor_version: "v1".into(),
            }),
            managed.clone(),
            100,
            2_000,
            16_000,
            60,
            false,
        )
        .unwrap(),
    );
    // 同一会话的三条事件在同一批内消费；循环到游标耗尽。
    let mut committed = 0u64;
    let mut skipped = 0u64;
    while let Some(run) = candidate_use_case.run_once().await.unwrap() {
        committed += run.candidates_committed;
        skipped += run.candidates_skipped;
    }
    assert_eq!(committed, 3, "all three events must be consumed");
    assert_eq!(skipped, 0);

    // 3. 重复扫描（游标回拨模拟崩溃后重启）不得重复建候选：fingerprint 幂等
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_memory_candidate_processing_state \
         SET last_received_at = NULL, last_source_event_id = NULL \
         WHERE account_id = (SELECT id FROM secretary_accounts \
           WHERE source_channel = ? AND platform_account_id = ?)",
        vec![
            MessageSource::NapCat.as_str().into(),
            managed_id.clone().into(),
        ],
    ))
    .await
    .unwrap();
    let mut rescan_committed = 0u64;
    while let Some(run) = candidate_use_case.run_once().await.unwrap() {
        rescan_committed += run.candidates_committed;
    }
    assert_eq!(
        rescan_committed, 0,
        "fingerprint dedup must reject duplicates across all conversations"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_candidates \
             WHERE account_id = (SELECT id FROM secretary_accounts \
               WHERE source_channel = ? AND platform_account_id = ?)",
            [MessageSource::NapCat.as_str(), managed_id.as_str()],
        )
        .await,
        3,
        "exactly three candidates after rescan"
    );

    // 4. Owner 列表查询：三类候选 proposed v1，来源精确
    let views = candidate_use_case
        .list(&managed, None, None, 100)
        .await
        .unwrap();
    assert_eq!(views.len(), 3);
    let commitment_view = views
        .iter()
        .find(|view| view.kind == MemoryCandidateKind::Commitment)
        .expect("commitment candidate must exist");
    assert_eq!(commitment_view.status, MemoryCandidateStatus::Proposed);
    assert_eq!(commitment_view.version.as_u64(), 1);
    // P0-2：承诺双方事件（promisor + beneficiary）都必须进入证据来源，
    // 因此候选来源数 = 2，而不是旧实现的 1。
    assert_eq!(commitment_view.source_excerpts.len(), 2);
    assert!(!commitment_view.conflicts_with_active_fact);
    let candidate_id = commitment_view.candidate_id.clone();
    let subject_key = commitment_view.subject_key.clone();

    // 5. OwnerCommand 与有效 OwnerBinding
    let command_account_id = format!("candidate-command-{suffix}");
    let command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "cand-cmd-1",
        "批准这个记忆候选",
        100_100,
    )
    .await;
    let command_source_event_id = SourceEventId::new(&command_event_id).unwrap();

    // 6. 初次运行：ApproveMemoryCandidate -> Suspend 等 Owner 审批
    let action_store = build_mysql_action_store(db.clone());
    let run_id = ActionRunId::for_owner_command(&command_source_event_id, "approve-v1");
    action_store
        .ensure_action_run(
            &run_id,
            &ActionRunSeed {
                account: managed.clone(),
                command_source_event_id: command_source_event_id.clone(),
                command_text: "批准这个记忆候选".into(),
                conversation_id: "owner-conv".into(),
                occurred_at_unix_secs: 100_100,
                timezone_offset_secs: 0,
                timezone: "UTC".into(),
                recent_events: vec![personal_secretary::RecentEventRef {
                    source_event_id: command_source_event_id.clone(),
                    summary: "Owner 命令".into(),
                }],
            },
        )
        .await
        .unwrap();
    let control = Arc::new(MemoryCandidateControlUseCase::new(
        build_mysql_memory_candidate_control_store(db.clone()),
    ));
    let initial = PlannerUseCase::new(
        action_store,
        Arc::new(ApproveMemoryCandidatePlanner {
            candidate_id: candidate_id.clone(),
            expected_version: 1,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_memory_candidate(Arc::clone(&candidate_use_case))
    .with_memory_candidate_control(Arc::clone(&control));
    let run = initial
        .run_once("test-worker")
        .await
        .unwrap()
        .expect("run must be claimed");
    assert!(run.suspended, "L2 approve must await owner approval");
    let checkpoint_id = run
        .checkpoint_id
        .expect("suspended run must have checkpoint");
    let proposal_id = run.proposal_id.expect("suspended run must have proposal");

    // 7. 模拟进程重建：全新 PlannerUseCase 与 CheckpointStore，Resume Approve
    let resumed = PlannerUseCase::new(
        build_mysql_action_store(db.clone()),
        Arc::new(ApproveMemoryCandidatePlanner {
            candidate_id: candidate_id.clone(),
            expected_version: 1,
        }),
        Arc::new(InMemoryCheckpointStore::<SecretaryAgentState>::new()),
        60,
    )
    .with_checkpoint_db(db.clone())
    .with_memory_candidate(Arc::clone(&candidate_use_case))
    .with_memory_candidate_control(control);
    let resumed_report = resumed
        .resume_run(
            &run_id,
            &checkpoint_id,
            SecretaryActionResumeInput {
                proposal_id: proposal_id.clone(),
                decision: SecretaryApprovalDecision::Approve,
                command_source_event_id: command_source_event_id.clone(),
                approval_source_event_id: None,
            },
        )
        .await
        .expect("approved resume must execute approve effect");
    assert!(
        resumed_report.completed,
        "approved resume must complete the run"
    );

    // 8. 精确一次：候选 approved 版本 +1、一条 Confirmed Fact（Commitment Pending）、
    //    一条来源、一条审计、一条 Receipt、一条响应
    assert_eq!(
        scalar_string(
            &db,
            "SELECT candidate_status AS value FROM secretary_memory_candidates \
             WHERE candidate_id = ?",
            [candidate_id.as_str()],
        )
        .await
        .as_deref(),
        Some("approved"),
        "candidate must be approved"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(candidate_version AS SIGNED) AS value \
             FROM secretary_memory_candidates WHERE candidate_id = ?",
            [candidate_id.as_str()],
        )
        .await,
        2,
        "approve must bump candidate version by exactly 1"
    );
    let fact_id = scalar_string(
        &db,
        "SELECT fact_id AS value FROM secretary_memory_facts \
         WHERE account_id = (SELECT id FROM secretary_accounts \
           WHERE source_channel = ? AND platform_account_id = ?) \
           AND fact_kind = 'commitment' AND subject_key = ?",
        [
            MessageSource::NapCat.as_str(),
            managed_id.as_str(),
            subject_key.as_str(),
        ],
    )
    .await
    .expect("approved candidate must create exactly one fact");
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_fact_sources \
             WHERE fact_id = ?",
            [fact_id.as_str()],
        )
        .await,
        2,
        "approved fact must carry the exact candidate sources (promisor + beneficiary)"
    );
    let fact_json = scalar_string(
        &db,
        "SELECT CAST(fact_json AS CHAR) AS value FROM secretary_memory_facts WHERE fact_id = ?",
        [fact_id.as_str()],
    )
    .await
    .expect("stored fact json");
    let fact: MemoryFact = serde_json::from_str(&fact_json).unwrap();
    assert_eq!(fact.status, MemoryFactStatus::Confirmed);
    let MemoryPayload::Commitment(commitment) = &fact.payload else {
        panic!("approved fact must be a commitment");
    };
    assert_eq!(
        commitment.status,
        CommitmentStatus::Pending,
        "approved commitment must be Pending for follow-up"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_candidate_controls \
             WHERE candidate_id = ? AND control_kind = 'approve'",
            [candidate_id.as_str()],
        )
        .await,
        1,
        "approve must write exactly one immutable control audit"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "approve must persist exactly one effect receipt"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_responses \
             WHERE run_id = ?",
            [run_id.as_str()],
        )
        .await,
        1,
        "completed resume must persist one owner response"
    );

    // 9. Commitment -> FollowUp 只生成一个：confirmed + Pending + due 在 horizon 内。
    //    断言 scoped 到本事实（全库扫描计数会被其他测试的数据污染）。
    let due_report = follow_up_scan_at(&db, 150_000).await;
    assert!(
        due_report.commitments_materialized >= 1,
        "approved commitment must be materialized into a follow-up"
    );
    assert_eq!(
        follow_up_count_for_fact(&db, fact_id.as_str()).await,
        1,
        "approved commitment must materialize exactly one follow-up"
    );
    let _ = follow_up_scan_at(&db, 150_000).await;
    assert_eq!(
        follow_up_count_for_fact(&db, fact_id.as_str()).await,
        1,
        "second scan must not duplicate the follow-up for this fact"
    );

    // 10. 第二次 Resume 必须被 Checkpoint CAS 拒绝，候选/审计/回执不再变化
    assert!(
        resumed
            .resume_run(
                &run_id,
                &checkpoint_id,
                SecretaryActionResumeInput {
                    proposal_id,
                    decision: SecretaryApprovalDecision::Approve,
                    command_source_event_id: command_source_event_id.clone(),
                    approval_source_event_id: None,
                },
            )
            .await
            .is_err(),
        "checkpoint CAS must reject the second approved resume"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(candidate_version AS SIGNED) AS value \
             FROM secretary_memory_candidates WHERE candidate_id = ?",
            [candidate_id.as_str()],
        )
        .await,
        2,
        "second resume must not move the candidate version again"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_candidate_controls \
             WHERE candidate_id = ?",
            [candidate_id.as_str()],
        )
        .await,
        1,
        "second resume must not write another audit"
    );
}
#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn memory_candidate_approval_rejects_cross_account_version_and_stale_sources() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("mcsec-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();

    // 六条独立会话的来源事件，分别承载六种候选
    let e1 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "sec-e1",
        "sec-g1",
        "alice",
        "人物：alice 是我同学",
        100_000,
    )
    .await;
    let e2 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "sec-e2",
        "sec-g2",
        "bob",
        "项目：beta 上线",
        100_001,
    )
    .await;
    let e3 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "sec-e3",
        "sec-g3",
        "alice",
        "承诺：给 bob 撤回测试",
        100_002,
    )
    .await;
    let e4 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "sec-e4",
        "sec-g4",
        "alice",
        "承诺：给 bob 长期记忆测试",
        100_003,
    )
    .await;
    let e5 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "sec-e5",
        "sec-g5",
        "alice",
        "承诺：给 bob 冲突测试",
        100_004,
    )
    .await;
    let e6 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "sec-e6",
        "sec-g6",
        "bob",
        "承诺：给 alice 拒绝测试",
        100_005,
    )
    .await;

    let c1 = format!("c1-{suffix}");
    let c2 = format!("c2-{suffix}");
    let c3 = format!("c3-{suffix}");
    let c4 = format!("c4-{suffix}");
    let c5 = format!("c5-{suffix}");
    let c6 = format!("c6-{suffix}");
    insert_candidate_row(
        &db,
        &managed,
        &c1,
        "person",
        "person:alice",
        MemoryPayload::Person(PersonMemory {
            person: ThreadActorRef {
                account: managed.clone(),
                actor_id: "alice".into(),
            },
            relationship: Some("同学".into()),
            responsibilities: Vec::new(),
            communication_preferences: Vec::new(),
        }),
        &e1,
        "alice",
    )
    .await;
    insert_candidate_row(
        &db,
        &managed,
        &c2,
        "project",
        "project:beta",
        MemoryPayload::Project(ProjectMemory {
            project_key: "beta".into(),
            goal: "上线".into(),
            member_actor_ids: Vec::new(),
            progress: None,
            decision_ids: Vec::new(),
            risks: Vec::new(),
            blockers: Vec::new(),
            artifact_refs: Vec::new(),
        }),
        &e2,
        "bob",
    )
    .await;
    let commitment_payload = |action: &str| {
        MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: managed.clone(),
                actor_id: "alice".into(),
            },
            beneficiary: ThreadActorRef {
                account: managed.clone(),
                actor_id: "bob".into(),
            },
            action: action.into(),
            due_at_unix_secs: None,
            status: CommitmentStatus::Proposed,
            completion_source_event_id: None,
        })
    };
    insert_candidate_row(
        &db,
        &managed,
        &c3,
        "commitment",
        "commitment:alice:bob:撤回测试",
        commitment_payload("撤回测试"),
        &e3,
        "alice",
    )
    .await;
    insert_candidate_row(
        &db,
        &managed,
        &c4,
        "commitment",
        "commitment:alice:bob:长期记忆测试",
        commitment_payload("长期记忆测试"),
        &e4,
        "alice",
    )
    .await;
    insert_candidate_row(
        &db,
        &managed,
        &c5,
        "commitment",
        "commitment:alice:bob:冲突测试",
        commitment_payload("冲突测试候选"),
        &e5,
        "alice",
    )
    .await;
    insert_candidate_row(
        &db,
        &managed,
        &c6,
        "commitment",
        "commitment:bob:alice:拒绝测试",
        MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: managed.clone(),
                actor_id: "bob".into(),
            },
            beneficiary: ThreadActorRef {
                account: managed.clone(),
                actor_id: "alice".into(),
            },
            action: "拒绝测试".into(),
            due_at_unix_secs: None,
            status: CommitmentStatus::Proposed,
            completion_source_event_id: None,
        }),
        &e6,
        "bob",
    )
    .await;

    // OwnerCommand + 绑定 + 已领取的 run（用于直接调用控制用例）
    let command_account_id = format!("mcsec-command-{suffix}");
    let command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "sec-cmd-1",
        "审批记忆候选",
        100_100,
    )
    .await;
    let command_source_event_id = SourceEventId::new(&command_event_id).unwrap();
    let action_store = build_mysql_action_store(db.clone());
    let (run_id, lease_token) =
        claim_control_run(&action_store, &command_source_event_id, &managed).await;
    let control =
        MemoryCandidateControlUseCase::new(build_mysql_memory_candidate_control_store(db.clone()));
    let candidate_store = build_mysql_memory_candidate_store(db.clone());

    // 1. 跨账号不可批准：其他账号的 run/命令/绑定 + 本账号候选 -> Unauthorized，零修改
    let other_id = format!("mcsec-other-{suffix}");
    let other_command_account_id = format!("mcsec-other-command-{suffix}");
    let other = SourceAccountRef::new(MessageSource::NapCat, &other_id).unwrap();
    // 先让"其他账号"存在一条消息（绑定 INSERT...SELECT 依赖 secretary_accounts 行）。
    insert_candidate_source_event(
        &inbound,
        &other_id,
        "sec-other-e1",
        "sec-other-g1",
        "carol",
        "其他账号的一条消息",
        100_200,
    )
    .await;
    let other_command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &other_id,
        &other_command_account_id,
        "sec-cmd-other-1",
        "审批他人候选",
        100_200,
    )
    .await;
    let other_command_source_event_id = SourceEventId::new(&other_command_event_id).unwrap();
    let (other_run_id, other_lease_token) =
        claim_control_run(&action_store, &other_command_source_event_id, &other).await;
    let cross_error = candidate_control_apply(
        &control,
        &other,
        &other_command_event_id,
        &other_run_id,
        &other_lease_token,
        "cross",
        SecretaryAction::ApproveMemoryCandidate {
            candidate_id: MemoryCandidateId::new(c1.clone()).unwrap(),
            expected_candidate_version: 1,
            reason: "跨账号批准".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(cross_error, MemoryCandidateControlStoreError::Unauthorized),
        "cross-account approve must be unauthorized, got {cross_error:?}"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT candidate_status AS value FROM secretary_memory_candidates \
             WHERE candidate_id = ?",
            [c1.as_str()],
        )
        .await
        .as_deref(),
        Some("proposed"),
        "cross-account attempt must not touch the candidate"
    );

    // 2. 版本错误：expected=5 但实际 1 -> InvalidData，零修改
    let version_error = candidate_control_apply(
        &control,
        &managed,
        &command_event_id,
        &run_id,
        &lease_token,
        "version",
        SecretaryAction::ApproveMemoryCandidate {
            candidate_id: MemoryCandidateId::new(c2.clone()).unwrap(),
            expected_candidate_version: 5,
            reason: "版本过期".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            version_error,
            MemoryCandidateControlStoreError::InvalidData(_)
        ),
        "stale version must be invalid data, got {version_error:?}"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(candidate_version AS SIGNED) AS value \
             FROM secretary_memory_candidates WHERE candidate_id = ?",
            [c2.as_str()],
        )
        .await,
        1,
        "stale version attempt must not move the version"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_candidate_controls \
             WHERE candidate_id = ?",
            [c2.as_str()],
        )
        .await,
        0,
        "stale version attempt must not write an audit"
    );

    // 3. 来源撤回：tombstone applied -> 批准失败 + 候选 invalidated 版本 +1
    let recall_event_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_recall_events \
         (recall_event_id, account_id, recall_kind, channel, conversation_kind, \
          platform_conversation_id, platform_message_id, correlation_key, \
          operator_platform_id, occurred_at_unix_secs) \
         SELECT ?, id, 'group', 'napcat', 'group', 'sec-g3', 'sec-e3', 'sec-e3', NULL, 105000 \
         FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
        vec![
            recall_event_id.clone().into(),
            MessageSource::NapCat.as_str().into(),
            managed_id.clone().into(),
        ],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_message_tombstones \
         (account_id, source_event_id, recall_event_id, channel, conversation_kind, \
          platform_conversation_id, platform_message_id, correlation_key, status, \
          invalidation_reason, invalidated_at_unix_secs) \
         SELECT id, ?, ?, 'napcat', 'group', 'sec-g3', 'sec-e3', 'sec-e3', 'applied', \
                '测试撤回', 105000 \
         FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
        vec![
            e3.clone().into(),
            recall_event_id.clone().into(),
            MessageSource::NapCat.as_str().into(),
            managed_id.clone().into(),
        ],
    ))
    .await
    .unwrap();
    let withdrawn_error = candidate_control_apply(
        &control,
        &managed,
        &command_event_id,
        &run_id,
        &lease_token,
        "withdrawn",
        SecretaryAction::ApproveMemoryCandidate {
            candidate_id: MemoryCandidateId::new(c3.clone()).unwrap(),
            expected_candidate_version: 1,
            reason: "批准已撤回来源".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            withdrawn_error,
            MemoryCandidateControlStoreError::InvalidData(_)
        ),
        "withdrawn source must fail approval, got {withdrawn_error:?}"
    );
    assert_eq!(
        candidate_store
            .invalidate_stale_proposed(&managed, 500)
            .await
            .unwrap(),
        1,
        "withdrawn candidate must be invalidated by stale-source scan"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT candidate_status AS value FROM secretary_memory_candidates \
             WHERE candidate_id = ?",
            [c3.as_str()],
        )
        .await
        .as_deref(),
        Some("invalidated"),
        "withdrawn candidate must become invalidated"
    );

    // 4. 会话切换为 never_long_term -> 批准失败 + invalidated
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_conversations SET memory_mode = 'never_long_term' \
         WHERE id = (SELECT conversation_id FROM secretary_source_events \
           WHERE source_event_id = ?)",
        [e4.clone().into()],
    ))
    .await
    .unwrap();
    let never_error = candidate_control_apply(
        &control,
        &managed,
        &command_event_id,
        &run_id,
        &lease_token,
        "never",
        SecretaryAction::ApproveMemoryCandidate {
            candidate_id: MemoryCandidateId::new(c4.clone()).unwrap(),
            expected_candidate_version: 1,
            reason: "批准已禁长期记忆来源".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            never_error,
            MemoryCandidateControlStoreError::InvalidData(_)
        ),
        "never_long_term source must fail approval, got {never_error:?}"
    );
    assert_eq!(
        candidate_store
            .invalidate_stale_proposed(&managed, 500)
            .await
            .unwrap(),
        1,
        "never_long_term candidate must be invalidated"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT candidate_status AS value FROM secretary_memory_candidates \
             WHERE candidate_id = ?",
            [c4.as_str()],
        )
        .await
        .as_deref(),
        Some("invalidated"),
        "never_long_term candidate must become invalidated"
    );

    // 5. 不同内容 active fact -> Conflict，不覆盖既有事实
    let memory = MemoryUseCase::new(build_mysql_memory_store(db.clone()));
    memory
        .remember(&MemoryFact {
            fact_id: MemoryFactId::generate(),
            account: managed.clone(),
            subject_key: "commitment:alice:bob:冲突测试".into(),
            payload: MemoryPayload::Commitment(CommitmentMemory {
                promisor: ThreadActorRef {
                    account: managed.clone(),
                    actor_id: "alice".into(),
                },
                beneficiary: ThreadActorRef {
                    account: managed.clone(),
                    actor_id: "bob".into(),
                },
                action: "完全不同的既有承诺".into(),
                due_at_unix_secs: None,
                status: CommitmentStatus::Pending,
                completion_source_event_id: None,
            }),
            status: MemoryFactStatus::Confirmed,
            confidence_bps: 9_500,
            source_event_ids: vec![SourceEventId::new(&e5).unwrap()],
            valid_until_unix_secs: None,
            supersedes_fact_id: None,
        })
        .await
        .unwrap();
    // 冲突是确定性业务结果：Receipt 必须包含旧 Fact ID 与 Candidate ID 的
    // 冲突响应，候选保持 proposed 且版本不变（供 Owner 后续决定拒绝或保留）。
    let conflict_receipt = candidate_control_apply(
        &control,
        &managed,
        &command_event_id,
        &run_id,
        &lease_token,
        "conflict",
        SecretaryAction::ApproveMemoryCandidate {
            candidate_id: MemoryCandidateId::new(c5.clone()).unwrap(),
            expected_candidate_version: 1,
            reason: "批准冲突候选".into(),
        },
    )
    .await
    .expect("conflict must complete as a business outcome, not a run failure");
    assert!(
        conflict_receipt.result_ref.contains(&"冲突".to_owned())
            && conflict_receipt.result_ref.contains(c5.as_str()),
        "conflict receipt must name candidate {}, got {}",
        c5,
        conflict_receipt.result_ref
    );
    let conflict_fact_id = scalar_string(
        &db,
        "SELECT fact_id AS value FROM secretary_memory_facts \
         WHERE account_id = (SELECT id FROM secretary_accounts \
           WHERE source_channel = ? AND platform_account_id = ?) \
           AND subject_key = 'commitment:alice:bob:冲突测试'",
        [MessageSource::NapCat.as_str(), managed_id.as_str()],
    )
    .await
    .expect("the conflicting fact must exist");
    assert!(
        conflict_receipt.result_ref.contains(&conflict_fact_id),
        "conflict receipt must name the old fact {}, got {}",
        conflict_fact_id,
        conflict_receipt.result_ref
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_facts \
             WHERE account_id = (SELECT id FROM secretary_accounts \
               WHERE source_channel = ? AND platform_account_id = ?) \
               AND subject_key = 'commitment:alice:bob:冲突测试'",
            [MessageSource::NapCat.as_str(), managed_id.as_str()],
        )
        .await,
        1,
        "conflict must not create or overwrite a fact"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT candidate_status AS value FROM secretary_memory_candidates \
             WHERE candidate_id = ?",
            [c5.as_str()],
        )
        .await
        .as_deref(),
        Some("proposed"),
        "conflict must leave the candidate proposed with version unchanged"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT control_kind AS value FROM secretary_memory_candidate_controls \
             WHERE candidate_id = ?",
            [c5.as_str()],
        )
        .await
        .as_deref(),
        Some("approve_conflict"),
        "conflict must write an approve_conflict audit row"
    );

    // 6. 拒绝：只写 rejected + 审计 + Receipt，不创建任何事实
    let reject_receipt = candidate_control_apply(
        &control,
        &managed,
        &command_event_id,
        &run_id,
        &lease_token,
        "reject",
        SecretaryAction::RejectMemoryCandidate {
            candidate_id: MemoryCandidateId::new(c6.clone()).unwrap(),
            expected_candidate_version: 1,
            reason: "Owner 判断该承诺不需要长期记忆".into(),
        },
    )
    .await
    .expect("reject must succeed");
    assert!(!reject_receipt.result_ref.is_empty());
    assert_eq!(
        scalar_string(
            &db,
            "SELECT candidate_status AS value FROM secretary_memory_candidates \
             WHERE candidate_id = ?",
            [c6.as_str()],
        )
        .await
        .as_deref(),
        Some("rejected"),
        "candidate must be rejected"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(candidate_version AS SIGNED) AS value \
             FROM secretary_memory_candidates WHERE candidate_id = ?",
            [c6.as_str()],
        )
        .await,
        2,
        "reject must bump candidate version by exactly 1"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_candidate_controls \
             WHERE candidate_id = ? AND control_kind = 'reject'",
            [c6.as_str()],
        )
        .await,
        1,
        "reject must write exactly one immutable control audit"
    );
    // scoped 到 reject 的 effect：同一 run 可能已由冲突检测写过 Receipt
    // （冲突是确定性业务结果），不能按 run_id 全量计数。
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_action_effect_receipts \
             WHERE effect_id = 'mc-effect-reject'",
            [],
        )
        .await,
        1,
        "reject must persist exactly one effect receipt"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_facts \
             WHERE account_id = (SELECT id FROM secretary_accounts \
               WHERE source_channel = ? AND platform_account_id = ?) \
               AND subject_key = 'commitment:bob:alice:拒绝测试'",
            [MessageSource::NapCat.as_str(), managed_id.as_str()],
        )
        .await,
        0,
        "reject must not create any memory fact"
    );
}

/// P0-1：跨会话交错事件（A1 -> B1 -> A2，全局按 received_at 递增）到达时，
/// 连续同会话前缀分批必须保证每条事件都被消费；回归：旧实现按 A1 的会话读取
/// 该会话全部事件并把全局游标推进到 A2，B1 被永久跳过。
#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn memory_candidate_interleaved_conversations_never_skip_events() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("inter-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();

    // 1. 三条 normal 事件交错在会话 A / B / A 之间，received_at 全局递增。
    let a1 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "inter-a1",
        "inter-group-a",
        "alice",
        "人物：alice 是客户",
        100_000,
    )
    .await;
    let b1 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "inter-b1",
        "inter-group-b",
        "bob",
        "项目：alpha 上线",
        100_001,
    )
    .await;
    let a2 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "inter-a2",
        "inter-group-a",
        "alice",
        "承诺：给 alice 发报价单",
        100_002,
    )
    .await;

    // 2. 连续消费到游标耗尽；每个批次只含连续同会话前缀（各 1 条）。
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let use_case = MemoryCandidateUseCase::new(
        build_mysql_memory_candidate_store(db.clone()),
        Arc::new(RecordingCandidateExtractor { seen: seen.clone() }),
        managed.clone(),
        100,
        2_000,
        16_000,
        60,
        false,
    )
    .unwrap();
    let mut runs = Vec::new();
    while let Some(run) = use_case.run_once().await.unwrap() {
        runs.push(run.events_read);
    }
    assert_eq!(
        runs,
        vec![1, 1, 1],
        "each batch must contain exactly one continuous same-conversation prefix"
    );
    let seen_events = seen.lock().unwrap().clone();
    assert_eq!(
        seen_events,
        vec![a1, b1, a2],
        "all interleaved events must reach the extractor in global order, none skipped"
    );
}

/// P0-2：审批复验必须绑定候选来源与权威事件发送者；来源 actor 与事件实际
/// actor 不一致时拒绝，候选/版本/审计/事实全部零修改。
#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn memory_candidate_approve_rejects_mismatched_source_actor() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("mism-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();

    // 1. 真实事件发送者是 alice；候选来源 actor 被篡改为 eve。
    let e1 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "mism-e1",
        "mism-group",
        "alice",
        "人物：alice 是客户",
        100_000,
    )
    .await;
    let command_account_id = format!("mism-command-{suffix}");
    let command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "mism-cmd-1",
        "审批记忆候选",
        100_100,
    )
    .await;
    let command_source_event_id = SourceEventId::new(&command_event_id).unwrap();
    let action_store = build_mysql_action_store(db.clone());
    let (run_id, lease_token) =
        claim_control_run(&action_store, &command_source_event_id, &managed).await;

    let control =
        MemoryCandidateControlUseCase::new(build_mysql_memory_candidate_control_store(db.clone()));
    let candidate_id = format!("mism-{}", &suffix[..8]);
    insert_candidate_row(
        &db,
        &managed,
        &candidate_id,
        "person",
        "person:alice",
        MemoryPayload::Person(PersonMemory {
            person: ThreadActorRef {
                account: managed.clone(),
                actor_id: "alice".into(),
            },
            relationship: None,
            responsibilities: Vec::new(),
            communication_preferences: Vec::new(),
        }),
        &e1,
        "eve",
    )
    .await;

    // 2. 批准必须因身份-证据不匹配而失败（InvalidData），零修改。
    let error = candidate_control_apply(
        &control,
        &managed,
        &command_event_id,
        &run_id,
        &lease_token,
        "mismatch",
        SecretaryAction::ApproveMemoryCandidate {
            candidate_id: MemoryCandidateId::new(candidate_id.clone()).unwrap(),
            expected_candidate_version: 1,
            reason: "审批篡改来源的候选".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, MemoryCandidateControlStoreError::InvalidData(_)),
        "mismatched source actor must be invalid data, got {error:?}"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT candidate_status AS value FROM secretary_memory_candidates \
             WHERE candidate_id = ?",
            [candidate_id.as_str()],
        )
        .await
        .as_deref(),
        Some("proposed"),
        "rejected approve must not move the candidate status"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(candidate_version AS SIGNED) AS value \
             FROM secretary_memory_candidates WHERE candidate_id = ?",
            [candidate_id.as_str()],
        )
        .await,
        1,
        "rejected approve must not bump the candidate version"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_candidate_controls \
             WHERE candidate_id = ?",
            [candidate_id.as_str()],
        )
        .await,
        0,
        "rejected approve must not write an audit"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_facts \
             WHERE account_id = (SELECT id FROM secretary_accounts \
               WHERE source_channel = ? AND platform_account_id = ?) \
               AND subject_key = 'person:alice'",
            [MessageSource::NapCat.as_str(), managed_id.as_str()],
        )
        .await,
        0,
        "rejected approve must not create any memory fact"
    );
}

/// 记录每次进入提取器输入的事件 ID；用于断言 local_only 事件从未到达提取器。
struct RecordingCandidateExtractor {
    seen: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl MemoryCandidateExtractorT for RecordingCandidateExtractor {
    async fn extract(
        &self,
        batch: &MemoryCandidateBatch,
    ) -> Result<Vec<MemoryCandidate>, MemoryCandidateExtractorError> {
        let mut seen = self.seen.lock().unwrap();
        for event in batch.events.iter().filter(|event| !event.content_omitted) {
            seen.push(event.source_event_id.as_str().to_owned());
        }
        // 不产出候选：本测试只关心"哪些事件进入了提取器输入"，而非候选内容。
        Ok(Vec::new())
    }
}

/// 远程 LLM 端点（allow_local_only=false）时，local_only 正文绝不能进入
/// 提取器输入；仅当端点验证为回环（allow_local_only=true）才可进入。
#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn memory_candidate_remote_llm_never_receives_local_only() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("trust-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();

    // 1. 一条 normal 群聊事件 + 一条随后切换为 local_only 的私聊事件
    let e_normal = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "trust-e1",
        "trust-normal-group",
        "alice",
        "人物：alice 是普通群成员",
        100_000,
    )
    .await;
    let e_local = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "trust-e2",
        "trust-local-group",
        "bob",
        "人物：bob 是私聊对象",
        100_001,
    )
    .await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_conversations SET memory_mode = 'local_only' \
         WHERE account_id = (SELECT id FROM secretary_accounts \
           WHERE source_channel = ? AND platform_account_id = ?) \
           AND platform_conversation_id = ?",
        vec![
            MessageSource::NapCat.as_str().into(),
            managed_id.clone().into(),
            "trust-local-group".into(),
        ],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_message_contents SET content_mode = 'local_only' \
         WHERE source_event_id = ?",
        vec![e_local.clone().into()],
    ))
    .await
    .unwrap();

    // 2. 远程 LLM：local_only 事件被 claim SQL 过滤，从未进入提取器输入
    let remote_store = build_mysql_memory_candidate_store(db.clone());
    let remote_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let remote = MemoryCandidateUseCase::new(
        remote_store,
        Arc::new(RecordingCandidateExtractor {
            seen: remote_seen.clone(),
        }),
        managed.clone(),
        100,
        2_000,
        16_000,
        60,
        false,
    )
    .unwrap();
    let run = remote
        .run_once()
        .await
        .unwrap()
        .expect("the normal event must be claimed");
    assert_eq!(run.events_read, 1, "batch must contain exactly one event");
    {
        // 块作用域释放 MutexGuard，确保后续 await 不持有锁。
        let seen = remote_seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "extractor must receive exactly one event");
        assert_eq!(
            seen[0], e_normal,
            "local_only event must never reach the extractor input with a remote LLM"
        );
    }
    assert!(
        remote.run_once().await.unwrap().is_none(),
        "no further claimable events while local_only is excluded"
    );

    // 3. 回环端点：local_only 事件现在可以进入提取器输入
    let local_store = build_mysql_memory_candidate_store(db.clone());
    let local_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let local = MemoryCandidateUseCase::new(
        local_store,
        Arc::new(RecordingCandidateExtractor {
            seen: local_seen.clone(),
        }),
        managed.clone(),
        100,
        2_000,
        16_000,
        60,
        true,
    )
    .unwrap();
    let run = local
        .run_once()
        .await
        .unwrap()
        .expect("the local_only event must be claimable with a loopback endpoint");
    assert_eq!(run.events_read, 1, "batch must contain exactly one event");
    {
        // 块作用域释放 MutexGuard。
        let seen = local_seen.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            [e_local.as_str()],
            "only the local_only event must reach the extractor input"
        );
    }
}

/// P0-6：local_only 事件在 normal 事件**之前**到达时，远程模式领取 normal 会把
/// 账号游标推进到 local_only 之后；切换本地模型后必须仍能领取被过滤事件——
/// 延期持久化防止游标永久越过（旧实现下 L1 不可达）。
#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn memory_candidate_local_only_before_normal_survives_remote_then_local() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("order-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();

    // 1. L1(local_only, 较早) + N1(normal, 较晚) 同会话。只把 L1 的正文模式降级
    //    为 local_only（会话模式两个事件共享，不能动），N1 保持 normal。
    let e_local = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "order-l1",
        "order-group",
        "alice",
        "人物：alice 是私聊客户",
        100_000,
    )
    .await;
    let e_normal = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "order-n1",
        "order-group",
        "bob",
        "项目：alpha 上线",
        100_001,
    )
    .await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_message_contents SET content_mode = 'local_only' \
         WHERE source_event_id = ?",
        vec![e_local.clone().into()],
    ))
    .await
    .unwrap();

    // 2. 远程模式：只领取 N1（游标越过 L1），L1 进入延期队列。
    let remote_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let remote = MemoryCandidateUseCase::new(
        build_mysql_memory_candidate_store(db.clone()),
        Arc::new(RecordingCandidateExtractor {
            seen: remote_seen.clone(),
        }),
        managed.clone(),
        100,
        2_000,
        16_000,
        60,
        false,
    )
    .unwrap();
    let run = remote
        .run_once()
        .await
        .unwrap()
        .expect("the normal event must be claimed");
    assert_eq!(
        run.events_read, 1,
        "remote batch must contain only the normal event"
    );
    {
        let seen = remote_seen.lock().unwrap();
        assert_eq!(seen.as_slice(), [e_normal.as_str()]);
    }
    assert!(
        remote.run_once().await.unwrap().is_none(),
        "no further claimable events in remote mode"
    );

    // 3. 切换本地模型：被过滤的 L1 必须仍可领取（延期消费，主游标不推进）。
    let local_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let local = MemoryCandidateUseCase::new(
        build_mysql_memory_candidate_store(db.clone()),
        Arc::new(RecordingCandidateExtractor {
            seen: local_seen.clone(),
        }),
        managed.clone(),
        100,
        2_000,
        16_000,
        60,
        true,
    )
    .unwrap();
    let run = local
        .run_once()
        .await
        .unwrap()
        .expect("the deferred local_only event must still be claimed");
    assert_eq!(
        run.events_read, 1,
        "deferred batch must contain exactly one event"
    );
    {
        let seen = local_seen.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            [e_local.as_str()],
            "deferred event must not be lost after the remote cursor advance"
        );
    }
    assert!(
        local.run_once().await.unwrap().is_none(),
        "no duplicate or leftover events after deferred drain"
    );

    // 4. 延期行已清理：切回远程也不产生重复处理。
    assert!(
        remote.run_once().await.unwrap().is_none(),
        "remote mode must stay idle after deferred drain"
    );
}

/// 批准与既有 active fact 内容一致的候选：引用既有事实并把新来源合并进
/// 来源链（P1-7），fact_sources 与 fact_json 同步，不丢失新证据。
#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated MySQL schema"]
async fn memory_candidate_approve_referenced_merges_new_sources() {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must be set for ignored MySQL test");
    let db = Database::connect(url).await.unwrap();
    apply_qqbot_migrations(&db).await;
    let inbound = build_mysql_inbound_event_store(db.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let managed_id = format!("merge-managed-{suffix}");
    let managed = SourceAccountRef::new(MessageSource::NapCat, &managed_id).unwrap();

    let e1 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "merge-e1",
        "merge-group",
        "alice",
        "承诺：给 bob 发送合并测试",
        100_000,
    )
    .await;
    let e2 = insert_candidate_source_event(
        &inbound,
        &managed_id,
        "merge-e2",
        "merge-group",
        "bob",
        "人物：bob 确认收到合并测试",
        100_001,
    )
    .await;
    let subject = "commitment:alice:bob:合并测试";
    let payload = || {
        MemoryPayload::Commitment(CommitmentMemory {
            promisor: ThreadActorRef {
                account: managed.clone(),
                actor_id: "alice".into(),
            },
            beneficiary: ThreadActorRef {
                account: managed.clone(),
                actor_id: "bob".into(),
            },
            action: "合并测试".into(),
            due_at_unix_secs: None,
            status: CommitmentStatus::Proposed,
            completion_source_event_id: None,
        })
    };

    // OwnerCommand + 绑定 + 已领取的 run
    let command_account_id = format!("merge-command-{suffix}");
    let command_event_id = owner_command_with_binding(
        &db,
        &inbound,
        &managed_id,
        &command_account_id,
        "merge-cmd-1",
        "批准记忆候选",
        100_100,
    )
    .await;
    let command_source_event_id = SourceEventId::new(&command_event_id).unwrap();
    let action_store = build_mysql_action_store(db.clone());
    let (run_id, lease_token) =
        claim_control_run(&action_store, &command_source_event_id, &managed).await;
    let control =
        MemoryCandidateControlUseCase::new(build_mysql_memory_candidate_control_store(db.clone()));

    // 1. 首次批准：形成新事实，来源 = e1
    let c1 = format!("mc1-{}", &suffix[..8]);
    insert_candidate_row(
        &db,
        &managed,
        &c1,
        "commitment",
        subject,
        payload(),
        &e1,
        "alice",
    )
    .await;
    let created = candidate_control_apply(
        &control,
        &managed,
        &command_event_id,
        &run_id,
        &lease_token,
        "merge-create",
        SecretaryAction::ApproveMemoryCandidate {
            candidate_id: MemoryCandidateId::new(c1.clone()).unwrap(),
            expected_candidate_version: 1,
            reason: "首次批准".into(),
        },
    )
    .await
    .expect("first approve must create the fact");
    assert!(
        created.result_ref.contains("已形成记忆"),
        "{}",
        created.result_ref
    );
    let fact_id = scalar_string(
        &db,
        "SELECT fact_id AS value FROM secretary_memory_facts \
         WHERE account_id = (SELECT id FROM secretary_accounts \
           WHERE source_channel = ? AND platform_account_id = ?) \
           AND subject_key = ?",
        [MessageSource::NapCat.as_str(), managed_id.as_str(), subject],
    )
    .await
    .expect("created fact must exist");
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_fact_sources \
             WHERE fact_id = ?",
            [fact_id.as_str()],
        )
        .await,
        1,
        "created fact must carry the first source only"
    );

    // 2. 内容一致的第二个候选：引用既有事实并合并 e2 进来源链
    let c2 = format!("mc2-{}", &suffix[..8]);
    insert_candidate_row(
        &db,
        &managed,
        &c2,
        "commitment",
        subject,
        payload(),
        &e2,
        "bob",
    )
    .await;
    let referenced = candidate_control_apply(
        &control,
        &managed,
        &command_event_id,
        &run_id,
        &lease_token,
        "merge-referenced",
        SecretaryAction::ApproveMemoryCandidate {
            candidate_id: MemoryCandidateId::new(c2.clone()).unwrap(),
            expected_candidate_version: 1,
            reason: "内容一致，引用既有事实".into(),
        },
    )
    .await
    .expect("same-content approve must reference the existing fact");
    assert!(
        referenced.result_ref.contains(&fact_id),
        "referenced receipt must name the existing fact, got {}",
        referenced.result_ref
    );

    // 3. 事实仍是同一条，来源链 = e1 + e2（不丢失新证据）
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_facts \
             WHERE account_id = (SELECT id FROM secretary_accounts \
               WHERE source_channel = ? AND platform_account_id = ?) \
               AND subject_key = ?",
            [MessageSource::NapCat.as_str(), managed_id.as_str(), subject],
        )
        .await,
        1,
        "same-content approve must not create a second fact"
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM secretary_memory_fact_sources \
             WHERE fact_id = ?",
            [fact_id.as_str()],
        )
        .await,
        2,
        "referenced approve must merge the new source into the fact source chain"
    );
    let fact_json = scalar_string(
        &db,
        "SELECT CAST(fact_json AS CHAR) AS value FROM secretary_memory_facts WHERE fact_id = ?",
        [fact_id.as_str()],
    )
    .await
    .expect("fact_json must be readable");
    assert!(
        fact_json.contains(&e1) && fact_json.contains(&e2),
        "fact_json must list both sources, got {fact_json}"
    );
    // 候选版本各自精确 +1
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT CAST(candidate_version AS SIGNED) AS value \
             FROM secretary_memory_candidates WHERE candidate_id = ?",
            [c2.as_str()],
        )
        .await,
        2,
        "referenced approve must bump the candidate version by exactly 1"
    );
}
