use std::sync::Arc;

use crate::app::community::community_service::CommunityService;
use crate::app::music::music_service::MusicService;

use super::BootstrapContext;

pub struct ContentServices {
    pub music: Arc<MusicService>,
    pub community: Arc<CommunityService>,
}

pub fn build_content_services(ctx: &BootstrapContext<'_>) -> ContentServices {
    let music = Arc::new(MusicService::new(Arc::clone(&ctx.repos.music_repo)));
    let community = Arc::new(CommunityService::new(Arc::clone(&ctx.repos.community_repo)));

    ContentServices { music, community }
}
