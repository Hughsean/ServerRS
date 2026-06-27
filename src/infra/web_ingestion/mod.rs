//! 网页知识摄取流水线的基础设施实现。
//!
//! 包含：
//! - SeaORM 仓库（全部 10 张表）
//! - HTTP fetcher with SSRF protection
//! - (LLM/embedding providers live in their own infrastructure modules)

pub mod distiller;
pub mod fetcher;
pub mod repo;
pub mod review_repository;
