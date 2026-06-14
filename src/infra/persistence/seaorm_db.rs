use sea_orm::{ConnectOptions, Database, DatabaseConnection};

/// Connects to the existing MySQL database. Does NOT create tables,
/// run migrations, or seed data — the database is the source of truth.
pub async fn init_db(
    database_url: &str,
    max_connections: u32,
) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let mut options = ConnectOptions::new(database_url.to_owned());
    options.max_connections(max_connections.max(1));
    let db = Database::connect(options).await?;
    tracing::info!("database connected");
    Ok(db)
}
