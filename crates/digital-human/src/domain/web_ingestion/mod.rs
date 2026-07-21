//! 网页知识摄取领域模块 — 状态常量、事件类型、 state machine,
//! value objects, error types, and repository traits.
//!
//! 设计原则：数据库状态机是权威事实来源。
//! Outbox events drive the pipeline. Each worker is idempotent.

pub mod distiller;
pub mod error;
pub mod event_types;
pub mod fetcher;
pub mod repo;
pub mod review;
pub mod state_machine;
pub mod status;

pub use distiller::*;
pub use error::*;
pub use event_types::*;
pub use fetcher::*;
pub use repo::*;
pub use review::*;
pub use state_machine::*;
pub use status::*;
