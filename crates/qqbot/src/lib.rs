//! QQBot 的 NapCat/OneBot 协议适配器。
//!
//! 当前 crate 不包含 QQ 业务流程、数据库实体或仓储实现。后续业务通过
//! [`napcat::NapCatEventHandler`] 接入协议事件。

pub mod napcat;
