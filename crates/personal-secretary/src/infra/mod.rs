mod repo;

use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::PersonalSecretaryStoreT;

pub fn build_mysql_inbound_event_store(db: DatabaseConnection) -> Arc<dyn PersonalSecretaryStoreT> {
    Arc::new(repo::MySqlInboundEventStore::new(db))
}
