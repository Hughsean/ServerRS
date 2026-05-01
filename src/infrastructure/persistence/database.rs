use sea_orm::{
    ActiveModelTrait, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, DbBackend,
    EntityTrait, PaginatorTrait, Schema,
};
use tracing::info;

/// Creates a MySQL connection via SeaORM and runs auto-migration.
pub async fn init_db(database_url: &str) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let db: DatabaseConnection = Database::connect(database_url).await?;

    let backend: DbBackend = DatabaseBackend::MySql;
    let schema = Schema::new(backend);

    // users
    db.execute(
        backend.build(
            schema
                .create_table_from_entity(super::entities::user::Entity)
                .if_not_exists(),
        ),
    )
    .await?;

    // user_profiles
    db.execute(
        backend.build(
            schema
                .create_table_from_entity(super::entities::user_profile::Entity)
                .if_not_exists(),
        ),
    )
    .await?;

    // conversations
    db.execute(
        backend.build(
            schema
                .create_table_from_entity(super::entities::conversation::Entity)
                .if_not_exists(),
        ),
    )
    .await?;

    // conversation_messages
    db.execute(
        backend.build(
            schema
                .create_table_from_entity(super::entities::conversation_message::Entity)
                .if_not_exists(),
        ),
    )
    .await?;

    seed_if_empty(&db).await?;

    info!("database initialised (MySQL) and schema applied");
    Ok(db)
}

async fn seed_if_empty(db: &DatabaseConnection) -> Result<(), sea_orm::DbErr> {
    use super::entities::user;

    let count = user::Entity::find().count(db).await?;
    if count > 0 {
        return Ok(());
    }

    let default_hash = bcrypt::hash("password123!", bcrypt::DEFAULT_COST)
        .expect("bcrypt default password hash should be generated");

    let now = chrono::Utc::now();

    let demo_user = user::ActiveModel {
        username: sea_orm::ActiveValue::Set("demo_user".into()),
        password: sea_orm::ActiveValue::Set(default_hash.clone()),
        email: sea_orm::ActiveValue::Set(None),
        phone: sea_orm::ActiveValue::Set(None),
        avatar: sea_orm::ActiveValue::Set(None),
        nickname: sea_orm::ActiveValue::Set(Some("Demo User".into())),
        status: sea_orm::ActiveValue::Set(1),
        created_at: sea_orm::ActiveValue::Set(now),
        updated_at: sea_orm::ActiveValue::Set(now),
        last_login_at: sea_orm::ActiveValue::Set(None),
        ..Default::default()
    };
    demo_user.insert(db).await?;

    let locked_user = user::ActiveModel {
        username: sea_orm::ActiveValue::Set("locked_user".into()),
        password: sea_orm::ActiveValue::Set(default_hash),
        email: sea_orm::ActiveValue::Set(None),
        phone: sea_orm::ActiveValue::Set(None),
        avatar: sea_orm::ActiveValue::Set(None),
        nickname: sea_orm::ActiveValue::Set(Some("Locked User".into())),
        status: sea_orm::ActiveValue::Set(0),
        created_at: sea_orm::ActiveValue::Set(now),
        updated_at: sea_orm::ActiveValue::Set(now),
        last_login_at: sea_orm::ActiveValue::Set(None),
        ..Default::default()
    };
    locked_user.insert(db).await?;

    info!("seeded demo users");
    Ok(())
}
