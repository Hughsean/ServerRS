use sea_orm::{
    ActiveModelTrait, ConnectionTrait, Database, DatabaseConnection, DbBackend, EntityTrait,
    PaginatorTrait, Schema,
};
use tracing::info;

/// Creates a MySQL connection via SeaORM and runs auto-migration.
pub async fn init_db(database_url: &str) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let db: DatabaseConnection = Database::connect(database_url).await?;

    let backend: DbBackend = DbBackend::MySql;
    let schema = Schema::new(backend);

    // users
    let stmt = schema
        .create_table_from_entity(super::entities::users::Entity)
        .if_not_exists()
        .to_owned();
    db.execute(&stmt).await?;

    // user_profiles
    let stmt = schema
        .create_table_from_entity(super::entities::user_profiles::Entity)
        .if_not_exists()
        .to_owned();
    db.execute(&stmt).await?;

    // conversations
    let stmt = schema
        .create_table_from_entity(super::entities::conversations::Entity)
        .if_not_exists()
        .to_owned();
    db.execute(&stmt).await?;

    // conversation_messages
    let stmt = schema
        .create_table_from_entity(super::entities::conversation_messages::Entity)
        .if_not_exists()
        .to_owned();
    db.execute(&stmt).await?;

    seed_if_empty(&db).await?;

    info!("database initialised (MySQL) and schema applied");
    Ok(db)
}

async fn seed_if_empty(db: &DatabaseConnection) -> Result<(), sea_orm::DbErr> {
    use super::entities::users;

    let count = users::Entity::find().count(db).await?;
    if count > 0 {
        return Ok(());
    }

    let default_hash = bcrypt::hash("password123!", bcrypt::DEFAULT_COST)
        .expect("bcrypt default password hash should be generated");

    let now = chrono::Utc::now();

    let demo_user = users::ActiveModel {
        username: sea_orm::ActiveValue::Set("demo_user".into()),
        password: sea_orm::ActiveValue::Set(default_hash.clone()),
        email: sea_orm::ActiveValue::Set(None),
        phone: sea_orm::ActiveValue::Set(None),
        avatar: sea_orm::ActiveValue::Set(None),
        nickname: sea_orm::ActiveValue::Set(Some("Demo User".into())),
        status: sea_orm::ActiveValue::Set(1_i8),
        created_at: sea_orm::ActiveValue::Set(now),
        updated_at: sea_orm::ActiveValue::Set(now),
        last_login_at: sea_orm::ActiveValue::Set(None),
        ..Default::default()
    };
    demo_user.insert(db).await?;

    let locked_user = users::ActiveModel {
        username: sea_orm::ActiveValue::Set("locked_user".into()),
        password: sea_orm::ActiveValue::Set(default_hash),
        email: sea_orm::ActiveValue::Set(None),
        phone: sea_orm::ActiveValue::Set(None),
        avatar: sea_orm::ActiveValue::Set(None),
        nickname: sea_orm::ActiveValue::Set(Some("Locked User".into())),
        status: sea_orm::ActiveValue::Set(0_i8),
        created_at: sea_orm::ActiveValue::Set(now),
        updated_at: sea_orm::ActiveValue::Set(now),
        last_login_at: sea_orm::ActiveValue::Set(None),
        ..Default::default()
    };
    locked_user.insert(db).await?;

    info!("seeded demo users");
    Ok(())
}
