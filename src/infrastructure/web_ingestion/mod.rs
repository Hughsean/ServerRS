//! Infrastructure implementations for the web ingestion pipeline.
//!
//! Contains:
//! - SeaORM repositories (all 10 tables)
//! - HTTP fetcher with SSRF protection
//! - (LLM/embedding providers live in their own infrastructure modules)

pub mod fetcher;
pub mod repositories;
