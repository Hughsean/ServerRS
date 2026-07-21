//! Digital-human business boundary: chat, memory, RAG, profile and tools.

pub mod app;
pub mod domain;
pub mod infra;
pub mod repositories;
pub mod shared;

#[cfg(test)]
mod test;
