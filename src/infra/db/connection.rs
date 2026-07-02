use sea_orm::{ConnectOptions, Database, DatabaseConnection};

/// 连接到现有的 MySQL 数据库。
///
/// 启动阶段只建立连接；表结构、迁移和初始化数据由数据库侧负责。
pub async fn init_db(
    database_url: &str,
    max_connections: u32,
) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let mut options = ConnectOptions::new(database_url.to_owned());
    options.max_connections(max_connections.max(1));
    let db = Database::connect(options).await?;
    tracing::info!("数据库已连接");
    Ok(db)
}
