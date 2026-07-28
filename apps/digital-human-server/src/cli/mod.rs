//! ServerRS CLI 客户端模块。
//!
//! 只通过 HTTP 调用后端,不依赖 domain/infra 层,保证与后端内部解耦。

pub mod audio_player;
pub mod auth;
pub mod client;
pub mod commands;
pub mod config;
pub mod dto;
pub mod error;
pub mod render;
pub mod repl;
