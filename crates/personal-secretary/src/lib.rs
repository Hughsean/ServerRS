//! 个人智能秘书的协议无关业务边界。
//!
//! 本 crate 只描述可信身份、对话、入站消息和指令权限，不依赖 NapCat、
//! QQ 开放平台、数据库或 Web 框架。

#[path = "application/action_graph/mod.rs"]
mod action_graph;
#[path = "application/agenda_service.rs"]
mod agenda_service;
#[path = "application/artifact_service.rs"]
mod artifact_service;
#[path = "application/backfill_service.rs"]
mod backfill_service;
#[path = "application/directory_service.rs"]
mod directory_service;
#[path = "application/follow_up_control_service.rs"]
mod follow_up_control_service;
#[path = "application/follow_up_service.rs"]
mod follow_up_service;
#[path = "application/health_service.rs"]
mod health_service;
#[path = "application/memory_candidate_service.rs"]
mod memory_candidate_service;
#[path = "application/memory_service.rs"]
mod memory_service;
#[path = "application/notification_policy_service.rs"]
mod notification_policy_service;
#[path = "application/planner_service.rs"]
mod planner_service;
#[path = "application/realtime_spool_service.rs"]
mod realtime_spool_service;
#[path = "application/recall_service.rs"]
mod recall_service;
#[path = "application/reconcile_service.rs"]
mod reconcile_service;
#[path = "application/response_expectation_control_service.rs"]
mod response_expectation_control_service;
#[path = "application/retriever_service.rs"]
mod retriever_service;
#[path = "application/store.rs"]
mod store;
#[path = "application/thread_control_service.rs"]
mod thread_control_service;
#[path = "application/thread_link_service.rs"]
mod thread_link_service;
#[path = "application/thread_mutation_service.rs"]
mod thread_mutation_service;
#[path = "application/thread_semantic_service.rs"]
mod thread_semantic_service;
#[path = "application/thread_service.rs"]
mod thread_service;

#[path = "domain/agenda.rs"]
mod agenda;
#[path = "domain/agent_runtime/mod.rs"]
mod agent_runtime;
#[path = "domain/artifact.rs"]
mod artifact;
#[path = "domain/backfill.rs"]
mod backfill;
#[path = "domain/continuity.rs"]
mod continuity;
#[path = "domain/directory.rs"]
mod directory;
#[path = "domain/follow_up.rs"]
mod follow_up;
#[path = "domain/health.rs"]
mod health;
#[path = "domain/inbound.rs"]
mod inbound;
#[path = "domain/memory.rs"]
mod memory;
#[path = "domain/memory_candidate.rs"]
mod memory_candidate;
#[path = "domain/notification_policy.rs"]
mod notification_policy;
#[path = "domain/planner.rs"]
mod planner;
#[path = "domain/realtime_spool.rs"]
mod realtime_spool;
#[path = "domain/recall.rs"]
mod recall;
#[path = "domain/retriever.rs"]
mod retriever;
#[path = "domain/thread_links.rs"]
mod thread_links;
#[path = "domain/thread_mutations.rs"]
mod thread_mutations;
#[path = "domain/thread_semantics.rs"]
mod thread_semantics;
#[path = "domain/threading.rs"]
mod threading;

pub use action_graph::{
    ActionGraphError, ActionGraphRuntime, ActionLeaseToken, ActionRunContext, ActionRunId,
    ActionRunSeed, ActionStoreError, ActionStoreT, ClaimedActionRun, L0ExecuteNode, NoActionNode,
    PlanNode, SecretaryActionEffectExecutor, SuspendedActionRun, SuspendedRunClaim, backoff_ms,
    build_action_graph, is_l0_direct_execute,
};
pub use agenda::{
    AgendaError, AgendaItem, AgendaItemId, AgendaItemKind, AgendaItemStatus, AgendaMutation,
    validate_agenda_mutation,
};
pub use agenda_service::{AgendaApplyRequest, AgendaMutationReceipt, AgendaStoreT, AgendaUseCase};
pub use agent_runtime::{
    AgentWorkingContextV1, FollowUpControlTarget, MAX_WORKING_BYTES, MAX_WORKING_EVIDENCE_REFS,
    MAX_WORKING_OPEN_REFERENCES, MAX_WORKING_RESOLVED_CONVERSATIONS, MAX_WORKING_RESOLVED_FACTS,
    MAX_WORKING_RESOLVED_PARTICIPANTS, MAX_WORKING_RESOLVED_THREADS, MAX_WORKING_TEXT_CHARS,
    MemoryCandidateConflictContext, MemoryConflictReasonCode, OpenReference, OpenReferenceKind,
    OwnerResponseDraft, RecentEventRef, ResponseExpectationControlTarget, ResponseSegment,
    RetrievalTriggerKind, SecretaryAction, SecretaryActionApprovalRequest, SecretaryActionEffect,
    SecretaryActionProposal, SecretaryActionReceipt, SecretaryActionResumeInput,
    SecretaryAgentPhase, SecretaryAgentRuntimeError, SecretaryAgentState, SecretaryAgentUpdate,
    SecretaryApprovalDecision, SecretaryRiskLevel, SecretaryToolKind, SecretaryToolPolicy,
    WorkingContextError, WorkingContextProjection, WorkingContextUpdate,
    build_action_response_draft, gate_secretary_action, summarize_memory_payload,
    validate_action_proposal, validate_response_draft, validate_working_context_projection,
};
pub use artifact::{
    ArtifactAvailability, ArtifactEnvelope, ArtifactError, ArtifactId, ArtifactKind,
    MAX_DESCRIPTION_CHARS, MAX_DISPLAY_NAME_CHARS, MAX_FORWARD_NESTING, MAX_HASH_CHARS,
    MAX_MIME_TYPE_CHARS, MAX_PLATFORM_REFERENCE_CHARS,
};
pub use artifact_service::{ArtifactStoreError, ArtifactStoreT, ArtifactUseCase};
pub use continuity::{
    ConnectionEndReason, ConnectionEpochId, ConnectionEpochStatus, ContinuityIdentityError,
    IngestionCursorScope, IngestionGapId, IngestionGapReason, IngestionGapStatus,
};
pub use directory::{
    ConversationScope, DirectoryError, DirectoryEvidence, DirectorySnapshot, DirectorySnapshotId,
    DirectorySourceApi, DirectoryStatus, ScopeBoundary, ScopeKind,
};
pub use directory_service::{
    DirectoryListEntry, DirectorySourceError, DirectorySourceT, DirectoryStoreError,
    DirectoryStoreT, DirectorySyncBudget, DirectorySyncError, DirectorySyncUseCase,
};
pub use follow_up::{
    ClaimedOwnerNotification, FollowUpId, FollowUpScanReport, FollowUpStatus,
    NotificationFailureKind, NotificationId, NotificationLeaseToken, OwnerNotificationContent,
    ResponseExpectationId,
};
pub use follow_up_control_service::{
    FollowUpControlEffectRequest, FollowUpControlStoreError, FollowUpControlStoreT,
    FollowUpControlUseCase,
};
pub use follow_up_service::{
    FollowUpStoreT, FollowUpUseCase, LegacyNotificationReconciliationConfig,
    LegacyNotificationReconciliationReport,
};
pub use health::{HealthSnapshot, HealthStatus, SubsystemHealth};
pub use health_service::{HealthAggregator, HealthSnapshotProducer};
pub use inbound::{
    ContentSegment, ConversationKind, ConversationRef, IdempotencyKey, InboundIdentityError,
    InboundMessageEnvelope, MediaKind, MessageRole, MessageSource, ObservedSenderProfile,
    RichContentKind, SourceAccountRef, SourceMessageRef, VerifiedActor, VerifiedActorKind,
};
pub use memory::{
    CommitmentMemory, CommitmentStatus, ConversationDerivedStateInvalidation,
    ConversationMemoryModeInput, ConversationMemoryModeReceipt, MemoryDeleteInput,
    MemoryDeleteReceipt, MemoryFact, MemoryFactError, MemoryFactId, MemoryFactStatus,
    MemoryFactView, MemoryPayload, MemorySourceExcerpt, MemoryWriteReceipt, PersonMemory,
    ProjectMemberRef, ProjectMemory, validate_memory_delete, validate_memory_fact,
    validate_memory_payload,
};
pub use memory_candidate::{
    APPROVED_CANDIDATE_CONFIDENCE_BPS, INITIAL_CANDIDATE_VERSION, MAX_CANDIDATE_PAYLOAD_BYTES,
    MAX_CANDIDATE_SOURCES, MemoryCandidate, MemoryCandidateBatch, MemoryCandidateCursor,
    MemoryCandidateError, MemoryCandidateEvent, MemoryCandidateId, MemoryCandidateKind,
    MemoryCandidateLeaseToken, MemoryCandidateSource, MemoryCandidateSourceExcerpt,
    MemoryCandidateStatus, MemoryCandidateVersion, MemoryCandidateView, candidate_fingerprint,
    candidate_to_confirmed_fact, is_eligible_for_candidate_extraction, validate_memory_candidate,
};
pub use memory_candidate_service::{
    ConservativeMemoryCandidateExtractor, MAX_INVALIDATE_PER_SCAN,
    MemoryCandidateControlEffectRequest, MemoryCandidateControlStoreError,
    MemoryCandidateControlStoreT, MemoryCandidateControlUseCase, MemoryCandidateExtractorError,
    MemoryCandidateExtractorT, MemoryCandidateRun, MemoryCandidateStoreT, MemoryCandidateUseCase,
    MemoryCandidateUseCaseError,
};
pub use memory_service::{MemoryStoreT, MemoryUseCase, MemoryUseCaseError};
pub use notification_policy::{
    ConversationMode, ConversationNotificationRule, DecisionReason, EvaluationInput,
    EvaluationPlan, EvaluationRequestId, EventKind, MAX_CANONICAL_SCOPE_KEY_BYTES,
    MAX_NOTIFICATION_AUDIT_SUMMARY_BYTES, MAX_NOTIFICATION_JSON_BYTES,
    MAX_NOTIFICATION_POLICY_ID_BYTES, MAX_NOTIFICATION_REASON_BYTES, MatchField,
    NotificationCandidateId, NotificationCandidateRef, NotificationCategory,
    NotificationDecisionId, NotificationMatchKeyV1, NotificationOutcome, NotificationPolicyError,
    NotificationPolicyEvaluator, NotificationPolicyFamily, NotificationPolicyKind,
    NotificationPolicyRevision, NotificationPolicyRule, PolicyFamilyId, PolicyRevisionId,
    QuietHoursRule, RevisionKind, StructuredImportance, validate_quiet_hours,
};
pub use notification_policy_service::{
    AutomaticReplyGateDecision, AutomaticReplyPolicyGate, ClaimedEvaluation, EvaluationCommit,
    EvaluationCommitResult, EvaluationSnapshot, FamilyGenerationSnapshot,
    MAX_EVALUATION_POLICY_FAMILIES, NotificationCandidateProductionReport,
    NotificationFeedbackRequest, NotificationPolicyAuthorizationContext,
    NotificationPolicyDisableRequest, NotificationPolicyEffectRequest,
    NotificationPolicyResponseArtifact, NotificationPolicyStoreError, NotificationPolicyStoreT,
    NotificationPolicyUseCase, NotificationPolicyUseCaseError, NotificationPolicyWriteRequest,
    OwnerBindingSnapshot, PolicyRuleSnapshot, authorize_notification_policy_action,
};
pub use planner::{
    ActionPlannerT, AgentEventView, AgentEventViewError, Clock, MemoryCandidateConflictResultV1,
    PlannerCommandEvent, PlannerError, PlannerInput, PlannerOutput, PlannerRetrievedExcerpt,
    PlannerToolObservation, QueryEffectResultV1, QueryEffectTypedEvent, SystemClock,
    TimeParseError, is_allowed_action_in_batch, is_allowed_after_memory_conflict,
    is_replan_observation_tool, naive_to_unix, parse_common_timezone_offset_secs,
    parse_datetime_with_timezone, parse_iso_datetime, validate_agent_event_view,
    validate_planner_input, validate_planner_output, validate_tool_observation,
};
pub use planner_service::{
    ActionCheckpointStoreFactoryT, PlannerRunReport, PlannerUseCase, PlannerUseCaseError,
};
pub use realtime_spool_service::RealtimeSpoolRecoveryStoreT;
pub use recall::{
    ClaimedRecallEvent, InvalidationTarget, RecallCorrelationKey, RecallError, RecallEvent,
    RecallEventId, RecallFailureKind, RecallKind, TombstoneRecord, TombstoneStatus,
};
pub use recall_service::{RecallStoreError, RecallStoreT, RecallUseCase};
pub use reconcile_service::{
    ClaimedPendingReply, ReconcileBudget, ReconcilePendingRepliesUseCase, ReconcileRunOutcome,
    ReplyReconcileStoreT,
};
pub use response_expectation_control_service::{
    ResponseExpectationControlEffectRequest, ResponseExpectationControlStoreError,
    ResponseExpectationControlStoreT, ResponseExpectationControlUseCase,
};
pub use retriever::{
    AccountScopedParticipantRef, CausalEventRef, CausalThreadRef, CommitmentQuery,
    CommitmentSummary, ContentTrustLevel, EventCausalContextView, EventParticipantSummary,
    EventQuery, EventRelation, EventRelationKind, EventSearchResult, GroupRole, IdentityTrust,
    MAX_ATTRIBUTE_VALUE_CHARS, MAX_CAUSAL_MENTIONED, MAX_CAUSAL_PARTICIPANTS, MAX_CAUSAL_RELATIONS,
    MAX_CAUSAL_SOURCE_REFS, MAX_PARTICIPANT_ALIASES, MAX_PARTICIPANT_ATTRIBUTES,
    MAX_PARTICIPANT_SOURCE_REFS, MAX_RELATED_EVENT_REFS, MAX_RELATION_SOURCES,
    ParticipantAttribute, ParticipantAttributeKind, ParticipantContextView, ParticipantIdentity,
    ParticipantRef, PendingOwnerWorkItem, PlatformIdentityKind, ProjectContextView,
    ProjectMemorySummary, ReferenceCandidate, ReferenceContext, ReferenceResolution,
    RetrievalVisibility, RetrieverError, RetrieverStoreT, SecretaryStatusView, SourceEventDetail,
    ThreadActorSummary, ThreadClaimSummary, ThreadContextView, ThreadDecisionRevisionCursor,
    ThreadDecisionRevisionPage, ThreadDecisionSummary, ThreadQuestionSummary,
    ThreadSearchMatchRank, ThreadSearchResult, UpcomingItem, check_causal_role_strictness,
    check_participant_permission_boundary, filter_for_model, grants_owner_authority,
    is_allowed_for_model, resolve_reference_from_candidates, validate_causal_context,
    validate_event_query, validate_participant_context,
};
pub use retriever_service::{RetrieverPolicy, RetrieverUseCase, RetrieverUseCaseError};
pub use store::{
    InboundEventStoreError, InboundEventStoreT, IngestMessageOutcome, IngestionContinuityStoreT,
    OwnerBinding, OwnerBindingStoreT, PersonalSecretaryStoreT, SourceEventId,
};
pub use thread_control_service::{
    ThreadControlEffectRequest, ThreadControlStoreError, ThreadControlStoreT, ThreadControlUseCase,
};
pub use thread_link_service::{
    ConservativeThreadLinkExtractor, ThreadLinkReviewReceipt, ThreadLinkReviewUseCase,
    ThreadLinkRun, ThreadLinkStoreT, ThreadLinkUseCase, ThreadLinkUseCaseError,
};
pub use thread_links::{
    ClaimedThreadLinkBatch, ThreadLinkCandidate, ThreadLinkCandidateCursor, ThreadLinkCandidateId,
    ThreadLinkCandidateStatus, ThreadLinkCandidateView, ThreadLinkConfidenceBand, ThreadLinkError,
    ThreadLinkEvent, ThreadLinkEvidence, ThreadLinkHint, ThreadLinkLeaseToken,
    ThreadLinkReviewAction, ThreadLinkReviewCommand, ThreadLinkReviewContext, ThreadLinkReviewId,
    ThreadLinkSignalKind, ThreadLinkSourceExcerpt, ValidatedThreadLinkReview,
    validate_thread_link_candidate, validate_thread_link_review,
};
pub use thread_mutation_service::{
    ThreadMutationApprovalNode, ThreadMutationDecisionNode, ThreadMutationEffectExecutor,
    ThreadMutationImpactRequest, ThreadMutationRevertUseCase, ThreadMutationStoreT,
    ThreadMutationUseCase, ThreadMutationUseCaseError,
};
pub use thread_mutations::{
    ThreadMutationAgentState, ThreadMutationApprovalRequest, ThreadMutationDecision,
    ThreadMutationEffect, ThreadMutationEffectReceipt, ThreadMutationError, ThreadMutationImpact,
    ThreadMutationKind, ThreadMutationProposalId, ThreadMutationProposalStatus,
    ThreadMutationResumeInput, ThreadMutationRevertInput, ThreadMutationRevertReceipt,
    ThreadMutationUpdate, suspend_thread_mutation_for_approval, validate_thread_mutation_impact,
    validate_thread_mutation_revert,
};
pub use thread_semantic_service::{
    ConservativeThreadSemanticExtractor, ThreadSemanticExtractorError, ThreadSemanticExtractorT,
    ThreadSemanticRun, ThreadSemanticStoreT, ThreadSemanticUseCase, ThreadSemanticUseCaseError,
};
pub use thread_semantics::{
    ClaimKind, ClaimedThreadSemanticBatch, LifecycleAuthority, OpenQuestionCandidate,
    ThreadClaimCandidate, ThreadDecisionCandidate, ThreadLifecycleChange,
    ThreadResolutionEvidenceKind, ThreadSemanticCursor, ThreadSemanticError, ThreadSemanticEvent,
    ThreadSemanticLeaseToken, ThreadSemanticPatch, ThreadStatusChangeId,
    classify_thread_resolution_evidence, derive_evidence_based_resolution, validate_semantic_patch,
    validate_thread_transition,
};
pub use thread_service::{
    ThreadProjectionError, ThreadProjectionRun, ThreadProjectionStoreT, ThreadProjectionUseCase,
};
pub use threading::{
    ClaimStatus, ClaimedThreadProjectionBatch, DecisionStatus, DeterministicThreadPlanner,
    DeterministicThreadPolicy, EventThread, EventThreadId, OpenQuestion, OpenQuestionId,
    QuestionStatus, ThreadActorRef, ThreadAssignment, ThreadClaim, ThreadClaimId,
    ThreadContextEvent, ThreadDecision, ThreadDecisionId, ThreadProjectionEvent,
    ThreadProjectionLeaseToken, ThreadProjectionPlan, ThreadRelation, ThreadRelationId,
    ThreadRelationKind, ThreadStatus, ThreadingError,
};

pub use backfill::{
    BackfillAnchor, BackfillAnomaly, BackfillBudget, BackfillConfigError, BackfillContinuation,
    BackfillCursor, BackfillError, BackfillEvidence, BackfillHistoryItem, BackfillLease,
    BackfillLeaseToken, BackfillOutcome, BackfillPage, BackfillReadDirection, BackfillRunId,
    BackfillRunProgress, BackfillRunStatus, BackfillScope, BackfillScopeStatus,
    BackfillSourceError, ClaimedGap, GapTransitionError, HistoryCompleteness, KnownScope,
    ReclaimPolicy, ScopeEvidence, ScopeProgress, validate_gap_transition,
};
pub use backfill_service::{
    BackfillGapUseCase, BackfillStateStoreT, BackfillStateStoreWithIngestionT,
    HistoryBackfillSourceT,
};
pub use realtime_spool::{
    ClaimedLegacyRealtimeSpoolEpoch, ConnectedEpochRecoveryStage, DurableSpoolReceipt,
    LegacyRealtimeSpoolEpoch, LegacyRealtimeSpoolRecoveryPlan, RealtimeSpoolAdmission,
    RealtimeSpoolAdmissionId, RealtimeSpoolAdmissionResult, RealtimeSpoolCheckpointEligibility,
    RealtimeSpoolCheckpointPrefix, RealtimeSpoolError, RealtimeSpoolFatal, RealtimeSpoolFatalKind,
    RealtimeSpoolGenerationId, RealtimeSpoolHookKey, RealtimeSpoolRecordId,
    RealtimeSpoolRecoveryFrame, RealtimeSpoolRecoveryLeaseToken, RealtimeSpoolRejection,
    RealtimeSpoolReplayProgress, RecoveredRealtimeSpoolFrame, checkpointable_prefix,
};

/// Graph CheckpointStore 的内存实现（仅测试用；生产用 MySQL 实现）。
pub use agent_core::graph::{CheckpointStore, InMemoryCheckpointStore};
