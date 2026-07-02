use std::sync::Arc;

use crate::app::community::community_service::CommunityService;
use crate::app::depression::depression_service::DepressionService;
use crate::app::diary::diary_service::DiaryService;
use crate::app::music::music_service::MusicService;
use crate::app::psychology::psychology_service::PsychologyService;
use crate::app::storage::object_service::ObjectService;
use crate::domain::storage::ObjectStorage;
use crate::infra::storage::local_storage::LocalObjectStorage;

use super::BootstrapContext;

pub struct DomainServices {
    pub objects: Arc<ObjectService>,
    pub psychology: Arc<PsychologyService>,
    pub depression: Arc<DepressionService>,
    pub diaries: Arc<DiaryService>,
    pub music: Arc<MusicService>,
    pub community: Arc<CommunityService>,
}

pub fn build_domain_services(ctx: &BootstrapContext<'_>) -> DomainServices {
    let psychology = Arc::new(PsychologyService::new(Arc::clone(
        &ctx.repos.psychology_repo,
    )));
    let depression = Arc::new(DepressionService::new(Arc::clone(
        &ctx.repos.depression_repo,
    )));
    let diaries = Arc::new(DiaryService::new(
        Arc::clone(&ctx.repos.diary_repo),
        Some(Arc::clone(&ctx.infra.ollama_provider)),
    ));
    let local_storage: Arc<dyn ObjectStorage> = Arc::new(LocalObjectStorage::new(
        std::path::PathBuf::from(&ctx.config.storage.base_path),
    ));
    let music = Arc::new(MusicService::new(Arc::clone(&ctx.repos.music_repo)));
    let community = Arc::new(CommunityService::new(Arc::clone(&ctx.repos.community_repo)));
    let objects = Arc::new(ObjectService::new(
        local_storage,
        Arc::clone(&ctx.repos.stored_object_repo),
        ctx.config.storage.clone(),
    ));

    DomainServices {
        objects,
        psychology,
        depression,
        diaries,
        music,
        community,
    }
}
