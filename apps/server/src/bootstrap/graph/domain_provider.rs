use std::sync::Arc;

use crate::app::community::community_service::CommunityService;
use crate::app::depression::depression_service::DepressionService;
use crate::app::diary::diary_service::DiaryService;
use crate::app::music::music_service::MusicService;
use crate::app::psychology::psychology_service::PsychologyService;
use crate::app::storage::object_service::ObjectService;

use super::BootstrapContext;
use super::content_provider::build_content_services;
use super::object_provider::build_object_services;
use super::wellbeing_provider::build_wellbeing_services;

pub struct DomainServices {
    pub objects: Arc<ObjectService>,
    pub psychology: Arc<PsychologyService>,
    pub depression: Arc<DepressionService>,
    pub diaries: Arc<DiaryService>,
    pub music: Arc<MusicService>,
    pub community: Arc<CommunityService>,
}

pub fn build_domain_services(ctx: &BootstrapContext<'_>) -> DomainServices {
    let object = build_object_services(ctx);
    let wellbeing = build_wellbeing_services(ctx);
    let content = build_content_services(ctx);

    DomainServices {
        objects: object.objects,
        psychology: wellbeing.psychology,
        depression: wellbeing.depression,
        diaries: wellbeing.diaries,
        music: content.music,
        community: content.community,
    }
}
