//! 个人智能秘书的协议无关业务边界。
//!
//! 本 crate 只描述可信身份、对话、入站消息和指令权限，不依赖 NapCat、
//! QQ 开放平台、数据库或 Web 框架。

mod agent_runtime;
mod continuity;
mod follow_up;
mod follow_up_service;
mod inbound;
mod memory;
mod memory_service;
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

pub use agent_runtime::{
    RecentEventRef, SecretaryAction, SecretaryActionApprovalRequest, SecretaryActionEffect,
    SecretaryActionProposal, SecretaryActionReceipt, SecretaryActionResumeInput,
    SecretaryAgentPhase, SecretaryAgentRuntimeError, SecretaryAgentState, SecretaryAgentUpdate,
    SecretaryApprovalDecision, SecretaryRiskLevel, SecretaryToolKind, SecretaryToolPolicy,
    gate_secretary_action, validate_action_proposal,
};
pub use continuity::{
    ConnectionEndReason, ConnectionEpochId, ConnectionEpochStatus, ContinuityIdentityError,
    IngestionCursorScope, IngestionGapId, IngestionGapReason, IngestionGapStatus,
};
pub use follow_up::{
    ClaimedOwnerNotification, FollowUpScanReport, FollowUpStatus, NotificationFailureKind,
    NotificationId, NotificationLeaseToken,
};
pub use follow_up_service::{FollowUpStoreT, FollowUpUseCase};
pub use inbound::{
    ContentSegment, ConversationKind, ConversationRef, IdempotencyKey, InboundIdentityError,
    InboundMessageEnvelope, MediaKind, MessageRole, MessageSource, SourceAccountRef,
    SourceMessageRef, VerifiedActor, VerifiedActorKind,
};
pub use memory::{
    CommitmentMemory, CommitmentStatus, MemoryDeleteInput, MemoryDeleteReceipt, MemoryFact,
    MemoryFactError, MemoryFactId, MemoryFactStatus, MemoryFactView, MemoryPayload,
    MemorySourceExcerpt, MemoryWriteReceipt, PersonMemory, ProjectMemory, validate_memory_delete,
    validate_memory_fact,
};
pub use memory_service::{MemoryStoreT, MemoryUseCase, MemoryUseCaseError};
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
    build_mysql_backfill_store, build_mysql_follow_up_store, build_mysql_inbound_event_store,
    build_mysql_memory_store, build_mysql_owner_binding_store, build_mysql_thread_link_store,
    build_mysql_thread_mutation_checkpoint_store, build_mysql_thread_mutation_store,
    build_mysql_thread_projection_store, build_mysql_thread_semantic_store,
};
