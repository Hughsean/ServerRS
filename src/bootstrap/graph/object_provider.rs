use std::sync::Arc;

use crate::app::storage::object_service::ObjectService;
use crate::domain::storage::ObjectStorage;
use crate::infra::storage::local_storage::LocalObjectStorage;

use super::BootstrapContext;

pub struct ObjectServices {
    pub objects: Arc<ObjectService>,
}

pub fn build_object_services(ctx: &BootstrapContext<'_>) -> ObjectServices {
    let local_storage: Arc<dyn ObjectStorage> = Arc::new(LocalObjectStorage::new(
        std::path::PathBuf::from(&ctx.config.storage.base_path),
    ));
    let objects = Arc::new(ObjectService::new(
        local_storage,
        Arc::clone(&ctx.repos.stored_object_repo),
        ctx.config.storage.clone(),
    ));

    ObjectServices { objects }
}
