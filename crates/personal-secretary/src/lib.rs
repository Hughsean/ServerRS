//! 个人智能秘书的协议无关业务边界。
//!
//! 本 crate 只描述可信身份、对话、入站消息和指令权限，不依赖 NapCat、
//! QQ 开放平台、数据库或 Web 框架。

mod continuity;
mod inbound;
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

pub use continuity::{
    ConnectionEndReason, ConnectionEpochId, ConnectionEpochStatus, ContinuityIdentityError,
    IngestionCursorScope, IngestionGapId, IngestionGapReason, IngestionGapStatus,
};
pub use inbound::{
    ContentSegment, ConversationKind, ConversationRef, IdempotencyKey, InboundIdentityError,
    InboundMessageEnvelope, MediaKind, MessageRole, MessageSource, SourceAccountRef,
    SourceMessageRef, VerifiedActor, VerifiedActorKind,
};
pub use store::{
    InboundEventStoreError, InboundEventStoreT, IngestMessageOutcome, IngestionContinuityStoreT,
    PersonalSecretaryStoreT, SourceEventId,
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
    ThreadMutationStoreT, ThreadMutationUseCase, ThreadMutationUseCaseError,
};
pub use thread_mutations::{
    ThreadMutationAgentState, ThreadMutationApprovalRequest, ThreadMutationDecision,
    ThreadMutationEffect, ThreadMutationEffectReceipt, ThreadMutationError, ThreadMutationImpact,
    ThreadMutationKind, ThreadMutationProposalId, ThreadMutationProposalStatus,
    ThreadMutationResumeInput, ThreadMutationUpdate, suspend_thread_mutation_for_approval,
    validate_thread_mutation_impact,
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
    build_mysql_backfill_store, build_mysql_inbound_event_store, build_mysql_thread_link_store,
    build_mysql_thread_mutation_checkpoint_store, build_mysql_thread_mutation_store,
    build_mysql_thread_projection_store, build_mysql_thread_semantic_store,
};
