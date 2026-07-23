//! 个人智能秘书的协议无关业务边界。
//!
//! 本 crate 只描述可信身份、对话、入站消息和指令权限，不依赖 NapCat、
//! QQ 开放平台、数据库或 Web 框架。

mod continuity;
mod inbound;
mod store;

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

pub use infra::build_mysql_inbound_event_store;
