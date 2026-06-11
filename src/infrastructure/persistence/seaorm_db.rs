use sea_orm::{Database, DatabaseConnection};

/// Connects to the existing MySQL database. Does NOT create tables,
/// run migrations, or seed data — the database is the source of truth.
pub async fn init_db(database_url: &str) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let db = Database::connect(database_url).await?;
    tracing::info!("database connected");
    Ok(db)
}
