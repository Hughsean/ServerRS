//! MySQL Action 仓储模块入口。
//!
//! 子模块按职责拆分（行为保持）：
//! - [`store`]：`MySqlActionStore` 与 `ActionStoreT` 实现（CAS、租约、事务）。
//! - [`queries`]：Row DTO、账号解析、领取行映射、Effect Receipt 查询。
//! - [`checkpoint`]：绑定业务 `ActionRunId` 的 `CheckpointStore`（CAS 单次消费）。
//!
//! `BIGINT UNSIGNED` 列用 `u64` 解码；MySQL JSON 列必须 `CAST(... AS CHAR)`。
//! 账号条件存在于数据库查询边界，而不只是上层过滤。

mod checkpoint;
mod queries;
mod store;

pub(crate) use checkpoint::BoundActionCheckpointStore;
pub(crate) use store::MySqlActionStore;
