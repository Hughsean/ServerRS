//! QQ business boundary: message lifecycle, group knowledge, NapCat and outbox.

pub mod app;
pub mod config;
pub mod domain;
pub mod infra;
pub mod repositories;
mod shared;

pub use config::QqBotConfig;
pub use domain::qq_bot::{
    AttentionStore, GroupMessageGateway, GroupMessageHandler, PlatformUserDirectory, QqBotError,
};
