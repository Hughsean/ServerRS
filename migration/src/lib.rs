pub use sea_orm_migration::prelude::*;

mod m20260705_000001_foundation_marker;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260705_000001_foundation_marker::Migration)]
    }
}
