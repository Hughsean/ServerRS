pub mod attention_store;
pub mod napcat;
pub mod repo;

// Re-export the error type for infra-level convenience.
pub use crate::domain::qq_bot::QqBotError;
