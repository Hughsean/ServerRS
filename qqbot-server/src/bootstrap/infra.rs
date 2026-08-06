//! 基础设施装配：数据库连接、入站事件仓储、账号引用与群白名单加载。
//!
//! 所有 Worker 装配之前必须完成白名单加载，避免文件读取失败时遗留 Worker。
//! 白名单为空表示不启用过滤（放行所有群）；非空集合内群号必须为正整数。

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use personal_secretary::{MessageSource, PersonalSecretaryStoreT, SourceAccountRef};
use personal_secretary_mysql::build_mysql_inbound_event_store;
use sea_orm::{ConnectOptions, Database};

use crate::config::AppConfig;
use crate::runtime::RuntimeError;

/// 基础设施装配产物：数据库连接、入站仓储、账号引用与群白名单。
///
/// 聚合为单一结构体，避免在入口层散落大量 `Arc::clone`。
pub(crate) struct InfraGraph {
    pub(crate) db: sea_orm::DatabaseConnection,
    pub(crate) store: Arc<dyn PersonalSecretaryStoreT>,
    pub(crate) account: SourceAccountRef,
    pub(crate) group_whitelist: Arc<HashSet<i64>>,
}

/// 连接 QQBot 独立 MySQL、构造入站事件仓储、解析账号引用并加载群白名单。
pub(crate) async fn assemble_infra(
    config: &AppConfig,
    config_dir: &Path,
) -> Result<InfraGraph, RuntimeError> {
    let mut database_options = ConnectOptions::new(config.database.url.clone());
    database_options.max_connections(config.database.max_connections.max(1));
    let db = Database::connect(database_options).await?;
    tracing::info!("个人秘书数据库已连接");

    let store = build_mysql_inbound_event_store(db.clone());
    let account =
        SourceAccountRef::new(MessageSource::NapCat, config.napcat.self_qq_id.to_string())?;

    // 在启动任何 Worker 之前加载群白名单，避免文件读取失败时遗留 Worker。
    let group_whitelist = Arc::new(
        config
            .whitelist
            .load_groups(config_dir)
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
    );
    if group_whitelist.is_empty() {
        tracing::info!("群白名单未启用（whitelist.whitelist_file 未配置），将处理所有群消息");
    } else {
        tracing::info!(
            group_count = group_whitelist.len(),
            "群白名单已启用，只处理白名单内群的消息"
        );
    }

    Ok(InfraGraph {
        db,
        store,
        account,
        group_whitelist,
    })
}
