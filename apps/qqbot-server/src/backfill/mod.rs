//! 历史回补应用层适配与独立 Worker。
//!
//! 本模块把外层基础设施（NapCat 只读客户端、回补状态仓储）装配到个人秘书的协议无关
//! 回补用例，并运行一个与实时 WebSocket 接收解耦的独立有界 Worker。分页算法、Gap
//! 完整性判定和 SQL 均不在本模块内，而在领域层与 MySQL 仓储中。

pub(crate) mod napcat_history_source;
pub(crate) mod worker;

#[allow(unused_imports)]
pub(crate) use worker::{BackfillHandle, spawn_backfill_worker};
