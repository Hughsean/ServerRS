//! 个人智能秘书的协议无关业务边界。
//!
//! 本 crate 只描述可信身份、对话、入站消息和指令权限，不依赖 NapCat、
//! QQ 开放平台、数据库或 Web 框架。

mod action_graph;
mod agenda;
mod agenda_service;
mod agent_runtime;
mod artifact;
mod artifact_service;
mod continuity;
mod directory;
mod directory_service;
mod follow_up;
mod follow_up_service;
mod health;
mod health_service;
mod inbound;
mod memory;
mod memory_service;
mod notification_policy;
mod notification_policy_service;
mod planner;
mod planner_service;
mod recall;
mod recall_service;
mod retriever;
mod retriever_service;
mod store;
mod thread_link_service;
mod thread_links;
mod thread_mutation_service;
mod thread_mutations;
mod thread_semantic_service;
mod thread_semantics;
mod thread_service;
mod threading;

mod backfill;
mod backfill_service;

mod infra;

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
    OwnerResponseDraft, RecentEventRef, ResponseSegment, SecretaryAction,
    SecretaryActionApprovalRequest, SecretaryActionEffect, SecretaryActionProposal,
    SecretaryActionReceipt, SecretaryActionResumeInput, SecretaryAgentPhase,
    SecretaryAgentRuntimeError, SecretaryAgentState, SecretaryAgentUpdate,
    SecretaryApprovalDecision, SecretaryRiskLevel, SecretaryToolKind, SecretaryToolPolicy,
    gate_secretary_action, validate_action_proposal, validate_response_draft,
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
    ClaimedOwnerNotification, FollowUpScanReport, FollowUpStatus, NotificationFailureKind,
    NotificationId, NotificationLeaseToken, OwnerNotificationContent,
};
pub use follow_up_service::{
    FollowUpStoreT, FollowUpUseCase, LegacyNotificationReconciliationConfig,
    LegacyNotificationReconciliationReport,
};
pub use health::{HealthSnapshot, HealthStatus, SubsystemHealth};
pub use health_service::{HealthAggregator, HealthSnapshotProducer};
pub use inbound::{
    ContentSegment, ConversationKind, ConversationRef, IdempotencyKey, InboundIdentityError,
    InboundMessageEnvelope, MediaKind, MessageRole, MessageSource, RichContentKind,
    SourceAccountRef, SourceMessageRef, VerifiedActor, VerifiedActorKind,
};
pub use memory::{
    CommitmentMemory, CommitmentStatus, MemoryDeleteInput, MemoryDeleteReceipt, MemoryFact,
    MemoryFactError, MemoryFactId, MemoryFactStatus, MemoryFactView, MemoryPayload,
    MemorySourceExcerpt, MemoryWriteReceipt, PersonMemory, ProjectMemory, validate_memory_delete,
    validate_memory_fact,
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
    ActionPlannerT, Clock, PlannerCommandEvent, PlannerError, PlannerInput, PlannerOutput,
    PlannerRetrievedExcerpt, SystemClock, TimeParseError, is_allowed_action_in_batch,
    naive_to_unix, parse_common_timezone_offset_secs, parse_datetime_with_timezone,
    parse_iso_datetime, validate_planner_input, validate_planner_output,
};
pub use planner_service::{PlannerRunReport, PlannerUseCase, PlannerUseCaseError};
pub use recall::{
    ClaimedRecallEvent, InvalidationTarget, RecallCorrelationKey, RecallError, RecallEvent,
    RecallEventId, RecallFailureKind, RecallKind, TombstoneRecord, TombstoneStatus,
};
pub use recall_service::{RecallStoreError, RecallStoreT, RecallUseCase};
pub use retriever::{
    ContentTrustLevel, EventQuery, EventSearchResult, IdentityTrust, ParticipantIdentity,
    ParticipantRef, PlatformIdentityKind, ReferenceCandidate, ReferenceContext,
    ReferenceResolution, RetrieverError, RetrieverStoreT, SourceEventDetail, ThreadSearchResult,
    UpcomingItem, filter_for_model, is_allowed_for_model, resolve_reference_from_candidates,
    validate_event_query,
};
pub use retriever_service::{RetrieverPolicy, RetrieverUseCase, RetrieverUseCaseError};
pub use store::{
    InboundEventStoreError, InboundEventStoreT, IngestMessageOutcome, IngestionContinuityStoreT,
    OwnerBinding, OwnerBindingStoreT, PersonalSecretaryStoreT, SourceEventId,
};
pub use thread_link_service::{
    ConservativeThreadLinkExtractor, ThreadLinkReviewReceipt, ThreadLinkReviewUseCase,
    ThreadLinkRun, ThreadLinkStoreT, ThreadLinkUseCase, ThreadLinkUseCaseError,
};
pub use thread_links::{
    ClaimedThreadLinkBatch, ThreadLinkCandidate, ThreadLinkCandidateCursor, ThreadLinkCandidateId,
    ThreadLinkCandidateStatus, ThreadLinkCandidateView, ThreadLinkError, ThreadLinkEvent,
    ThreadLinkEvidence, ThreadLinkHint, ThreadLinkLeaseToken, ThreadLinkReviewAction,
    ThreadLinkReviewCommand, ThreadLinkReviewContext, ThreadLinkReviewId, ThreadLinkSignalKind,
    ThreadLinkSourceExcerpt, ValidatedThreadLinkReview, validate_thread_link_candidate,
    validate_thread_link_review,
};
pub use thread_mutation_service::{
    ThreadMutationApprovalNode, ThreadMutationDecisionNode, ThreadMutationEffectExecutor,
    ThreadMutationRevertUseCase, ThreadMutationStoreT, ThreadMutationUseCase,
    ThreadMutationUseCaseError,
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
    ThreadClaimCandidate, ThreadDecisionCandidate, ThreadLifecycleChange, ThreadSemanticCursor,
    ThreadSemanticError, ThreadSemanticEvent, ThreadSemanticLeaseToken, ThreadSemanticPatch,
    ThreadStatusChangeId, validate_semantic_patch, validate_thread_transition,
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
    BackfillAnchor, BackfillAnomaly, BackfillBudget, BackfillConfigError, BackfillCursor,
    BackfillError, BackfillEvidence, BackfillHistoryItem, BackfillLease, BackfillLeaseToken,
    BackfillOutcome, BackfillPage, BackfillRunId, BackfillRunProgress, BackfillRunStatus,
    BackfillScope, BackfillScopeStatus, BackfillSourceError, ClaimedGap, GapTransitionError,
    HistoryCompleteness, KnownScope, ReclaimPolicy, ScopeEvidence, ScopeProgress,
    validate_gap_transition,
};
pub use backfill_service::{
    BackfillGapUseCase, BackfillStateStoreT, BackfillStateStoreWithIngestionT,
    HistoryBackfillSourceT,
};

pub use infra::{
    build_bound_action_checkpoint_store, build_mysql_action_store, build_mysql_agenda_store,
    build_mysql_artifact_store, build_mysql_backfill_store, build_mysql_directory_store,
    build_mysql_follow_up_store, build_mysql_inbound_event_store, build_mysql_memory_store,
    build_mysql_notification_policy_store, build_mysql_owner_binding_store,
    build_mysql_recall_store, build_mysql_retriever_store, build_mysql_thread_link_store,
    build_mysql_thread_mutation_checkpoint_store, build_mysql_thread_mutation_store,
    build_mysql_thread_projection_store, build_mysql_thread_semantic_store,
};

/// Graph CheckpointStore 的内存实现（仅测试用；生产用 MySQL 实现）。
pub use agent_core::graph::{CheckpointStore, InMemoryCheckpointStore};
