//! 启动装配：把依赖构造从入口层下沉，保持 `runtime::run` 只表达启动顺序。
//!
//! - [`infra`]：数据库连接、入站事件仓储、账号引用与群白名单加载。
//! - [`workers`]：Worker 句柄聚合与 QQ 开放平台装配。
//! - [`thread_pipeline`]：线程投影/语义/关联/历史回补装配。
//! - [`action_planner`]：Action Planner（含 LLM 与 NoAction 降级）装配。

pub(crate) mod action_planner;
pub(crate) mod agenda;
pub(crate) mod infra;
pub(crate) mod thread_pipeline;
pub(crate) mod workers;
